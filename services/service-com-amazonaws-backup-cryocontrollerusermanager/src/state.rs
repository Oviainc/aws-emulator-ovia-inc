// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-backup-cryocontrollerusermanager service.
//!
//! The vault / backup-job slice of AWS Backup is modelled here as three records
//! plus one index, so every operation computes its answer from the SAME stored
//! resource:
//!
//! * [`BackupVault`] — what `CreateBackupVault` inserts and `DeleteBackupVault`
//!   removes.
//! * [`RecoveryPoint`] — what `StartBackupJob` stores inside the named vault.
//! * [`BackupJob`] — the job that produced that recovery point.
//!
//! `recovery_points_by_vault` is the ONLY per-vault list: both
//! `ListRecoveryPointsByBackupVault`'s page and `DescribeBackupVault`'s
//! `NumberOfRecoveryPoints` are derived from it, so the two can never disagree.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::Lifecycle;

/// The ARN namespace this service stamps into every ARN it mints
/// (`OPERATIONS.md`: "ARN namespace: `backup`").
const ARN_NAMESPACE: &str = "backup";

/// The region an ARN carries when the request is unsigned and therefore names
/// no SigV4 credential scope to read a region from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `VaultType` for an ordinary backup vault (model enum `VaultType`).
pub const VAULT_TYPE_BACKUP_VAULT: &str = "BACKUP_VAULT";
/// `VaultState` of a vault that is ready to receive backups (model enum `VaultState`).
pub const VAULT_STATE_AVAILABLE: &str = "AVAILABLE";
/// `EncryptionKeyType` when the caller supplied no `EncryptionKeyArn`.
pub const ENCRYPTION_KEY_TYPE_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";
/// `EncryptionKeyType` when the caller supplied an `EncryptionKeyArn`.
pub const ENCRYPTION_KEY_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// `BackupJobState` a freshly started job is in (model enum `BackupJobState`).
pub const BACKUP_JOB_STATE_CREATED: &str = "CREATED";
/// `BackupJobState` a settled job is in.
pub const BACKUP_JOB_STATE_COMPLETED: &str = "COMPLETED";
/// `RecoveryPointStatus` of a recovery point whose job has not settled yet.
pub const RECOVERY_POINT_STATUS_CREATING: &str = "CREATING";
/// `RecoveryPointStatus` of a recovery point whose job has settled.
pub const RECOVERY_POINT_STATUS_COMPLETED: &str = "COMPLETED";
/// `IndexStatus` of a recovery point whose index has not been built yet.
pub const INDEX_STATUS_PENDING: &str = "PENDING";
/// `IndexStatus` of a recovery point whose index is queryable.
pub const INDEX_STATUS_ACTIVE: &str = "ACTIVE";

/// `PercentDone` of a job that has not settled yet.
pub const PERCENT_DONE_NONE: &str = "0.0";
/// `PercentDone` of a settled job.
pub const PERCENT_DONE_COMPLETE: &str = "100.0";

/// The page size a list operation uses when the caller names no `MaxResults`:
/// the model's own `MaxResults` maximum (`@range(min: 1, max: 1000)`), so one
/// unpaginated call returns everything a paginated one could.
pub const MAX_PAGE_SIZE: usize = 1000;

/// The maximum either backup window accepts, in minutes: 100 years, as the
/// model documents for both members (`StartWindowMinutes`: "This parameter has
/// a maximum value of 100 years (52,560,000 minutes)"; `CompleteWindowMinutes`:
/// "Like StartWindowMinutes, this parameter has a maximum value of 100 years
/// (52,560,000 minutes)").
pub const MAX_WINDOW_MINUTES: i64 = 52_560_000;

/// The minimum `StartWindowMinutes` accepts, in minutes, as the model documents
/// ("If this value is included, it must be at least 60 minutes to avoid
/// errors").
pub const MIN_START_WINDOW_MINUTES: i64 = 60;

/// The most backup vaults this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and
/// every request can add to it, so the slice answers the modelled
/// `LimitExceededException` past this point rather than growing without bound.
/// AWS enforces a per-account vault quota of its own, so that modelled error is
/// the right answer to the same request there.
pub const MAX_VAULTS: usize = 500;

/// The most recovery points this emulator keeps across all vaults. An
/// EMULATOR-CHOSEN ceiling, for the reason given on [`MAX_VAULTS`]; AWS caps
/// concurrent backup jobs per account and answers `LimitExceededException` too.
pub const MAX_RECOVERY_POINTS: usize = 10_000;

/// The most backup jobs this emulator keeps. An EMULATOR-CHOSEN ceiling, for
/// the reason given on [`MAX_VAULTS`].
pub const MAX_BACKUP_JOBS: usize = 10_000;

/// The most `IdempotencyToken` records this emulator keeps. An EMULATOR-CHOSEN
/// ceiling: the key is a string the CALLER picks, so this is the one store a
/// caller controls the keys of. Past the ceiling the oldest record is dropped —
/// a dropped token simply stops replaying, which is the same answer a caller
/// gets for a token it never sent.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1_000;

/// How long a recorded `IdempotencyToken` stays replayable. An
/// EMULATOR-CHOSEN span: AWS documents no lifetime for Backup's idempotency
/// tokens, and an in-memory store that never forgets a caller-chosen key grows
/// with the request count.
pub const IDEMPOTENCY_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after `StartBackupJob` the emulated job settles from `CREATED` to
/// `COMPLETED`.
///
/// A CHOSEN span, not a documented one: AWS takes as long as copying the
/// protected resource needs. It is deliberately non-zero so a caller that
/// describes the job straight after starting it still observes the transient
/// `CREATED` state, and the due instant is stored on the job
/// ([`BackupJob::settles_at`]) so no handler ever sleeps for it.
pub const JOB_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// A seconds-per-day constant for turning `Lifecycle` day counts into the
/// instants `CalculatedLifecycle` reports.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The ARN of a backup vault: `arn:aws:backup:<region>:<account>:backup-vault:<name>`.
pub fn backup_vault_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("backup-vault:{name}"))
}

/// The ARN of a recovery point: `arn:aws:backup:<region>:<account>:recovery-point:<id>`.
pub fn recovery_point_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("recovery-point:{id}"))
}

/// The region this request's ARNs carry: the one the caller signed for, from the
/// SigV4 credential scope, falling back to [`DEFAULT_REGION`] for an unsigned
/// request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Encode a pagination cursor: the key of the first item of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller and safe to carry in a
/// `@httpQuery` parameter whatever the key holds (a recovery-point ARN holds
/// both `:` and `/`).
pub fn encode_page_token(key: &str) -> String {
    hex::encode(key)
}

/// Decode a cursor produced by [`encode_page_token`]; `None` when the token was
/// not produced by this service.
pub fn decode_page_token(token: &str) -> Option<String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Whether `value` satisfies the model's `ResourceType` pattern
/// (`^[a-zA-Z0-9\-\_\.]{1,50}$`), so a derived resource type is never reported
/// in a form the model refuses.
fn is_modelled_resource_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 50
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Whether `value` satisfies the model's `BackupVaultName` pattern
/// (`^[a-zA-Z0-9\-\_]{2,50}$`).
///
/// The operations that take the vault name in the URI path (`CreateBackupVault`,
/// `ListRecoveryPointsByBackupVault`) target the patterned `BackupVaultName`
/// shape, so the framework refuses a bad name for them before any handler runs.
/// `DescribeBackupVault` and `DeleteBackupVault` target a plain `string`, so
/// they apply the same rule here rather than reporting a name the service could
/// never have stored as merely absent.
pub fn is_modelled_vault_name(value: &str) -> bool {
    (2..=50).contains(&value.chars().count())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `value` is structurally an ARN: the `arn` scheme and the six
/// colon-separated fields `arn:<partition>:<service>:<region>:<account>:<resource>`
/// every AWS ARN carries (the resource field may itself contain `:`).
///
/// The model types these members as `ARN` with no pattern, so the framework
/// cannot refuse a malformed one; AWS does, with
/// `InvalidParameterValueException`.
pub fn is_arn(value: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// The Backup resource type of a protected resource, derived from its ARN.
///
/// AWS Backup names the protected resource's type after the service that owns
/// the ARN; the catalog below is the subset of the resource types the Backup
/// API documents (`ListBackupJobs` `ByResourceType`) that the model's
/// `ResourceType` pattern admits — the pattern forbids the space in the
/// documented `Storage Gateway` / `SAP HANA on Amazon EC2` spellings, so those
/// namespaces fall through to the rule below. A namespace the catalog does not
/// name is reported as the namespace itself, which is what the ARN says about
/// the resource; anything the pattern refuses is reported as absent.
pub fn resource_type_of(resource_arn: &str) -> Option<String> {
    let mut fields = resource_arn.split(':');
    let (Some("arn"), Some(_partition), Some(service)) =
        (fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    // Skip region and account; what is left is the resource portion, which may
    // itself contain `:`.
    let resource = fields.skip(2).collect::<Vec<_>>().join(":");
    let resource_type = match service {
        "dynamodb" => "DynamoDB",
        "elasticfilesystem" => "EFS",
        "fsx" => "FSx",
        "s3" => "S3",
        "neptune" => "Neptune",
        "docdb" => "DocumentDB",
        "redshift" => "Redshift",
        "timestream" => "Timestream",
        "cloudformation" => "CloudFormation",
        // One namespace, two Backup resource types: an RDS cluster is Aurora,
        // a single instance is RDS.
        "rds" => {
            if resource.starts_with("cluster") {
                "Aurora"
            } else {
                "RDS"
            }
        }
        // Likewise: an EC2 volume is backed up as EBS, an instance as EC2.
        "ec2" => {
            if resource.starts_with("volume/") {
                "EBS"
            } else {
                "EC2"
            }
        }
        other => other,
    };
    is_modelled_resource_type(resource_type).then(|| resource_type.to_string())
}

/// A backup vault: the logical container `CreateBackupVault` inserts.
#[derive(Debug, Clone)]
pub struct BackupVault {
    pub name: String,
    pub arn: String,
    pub encryption_key_arn: Option<String>,
    pub creator_request_id: Option<String>,
    pub creation_date: SystemTime,
    /// Always [`VAULT_TYPE_BACKUP_VAULT`] for a vault made by `CreateBackupVault`;
    /// the other `VaultType` values belong to operations still at 501.
    pub vault_type: String,
    pub vault_state: String,
    pub tags: BTreeMap<String, String>,
    pub locked: bool,
    pub min_retention_days: Option<i64>,
    pub max_retention_days: Option<i64>,
}

impl BackupVault {
    /// The `EncryptionKeyType` this vault reports: customer-managed exactly when
    /// the create request named a key.
    pub fn encryption_key_type(&self) -> &'static str {
        if self.encryption_key_arn.is_some() {
            ENCRYPTION_KEY_TYPE_CUSTOMER_MANAGED
        } else {
            ENCRYPTION_KEY_TYPE_AWS_OWNED
        }
    }
}

/// A recovery point stored inside a vault by `StartBackupJob`.
#[derive(Debug, Clone)]
pub struct RecoveryPoint {
    pub arn: String,
    pub backup_vault_name: String,
    pub backup_vault_arn: String,
    pub resource_arn: String,
    pub resource_type: Option<String>,
    pub iam_role_arn: String,
    pub status: String,
    pub creation_date: SystemTime,
    /// When the clock owes this recovery point its `CREATING` -> `COMPLETED`
    /// transition. See [`JOB_SETTLE_AFTER`].
    pub settles_at: SystemTime,
    pub completion_date: Option<SystemTime>,
    pub backup_size_in_bytes: i64,
    pub lifecycle: Option<Lifecycle>,
    pub encryption_key_arn: Option<String>,
    pub is_encrypted: bool,
    pub is_parent: bool,
    pub tags: BTreeMap<String, String>,
    /// `IndexStatus`, set only when `StartBackupJob` asked for an index
    /// (`Index: ENABLED`); it settles from `PENDING` to `ACTIVE` alongside the
    /// job that built it.
    pub index_status: Option<String>,
    /// The id of the `StartBackupJob` that created it.
    pub created_by_job_id: String,
}

impl RecoveryPoint {
    /// The instants the stored `Lifecycle` makes due, as
    /// `CalculatedLifecycle` reports them: the day counts the caller gave
    /// counted forward from this recovery point's creation.
    pub fn calculated_lifecycle(&self) -> Option<(Option<SystemTime>, Option<SystemTime>)> {
        let lifecycle = self.lifecycle.as_ref()?;
        let after = |days: Option<i64>| -> Option<SystemTime> {
            // `-1` is the documented sentinel for "keep the recovery point
            // indefinitely" (model doc for `Lifecycle`), so it makes nothing
            // due; and a day count too large to express as an instant has
            // nothing due either. Both answer `None` rather than overflowing.
            let days = u64::try_from(days?).ok()?;
            let seconds = days.checked_mul(SECONDS_PER_DAY)?;
            self.creation_date.checked_add(Duration::from_secs(seconds))
        };
        Some((
            after(lifecycle.move_to_cold_storage_after_days),
            after(lifecycle.delete_after_days),
        ))
    }

    /// The `EncryptionKeyType` this recovery point reports, derived from the key
    /// it is stored under.
    pub fn encryption_key_type(&self) -> &'static str {
        if self.encryption_key_arn.is_some() {
            ENCRYPTION_KEY_TYPE_CUSTOMER_MANAGED
        } else {
            ENCRYPTION_KEY_TYPE_AWS_OWNED
        }
    }
}

/// A backup job: what `StartBackupJob` starts and `DescribeBackupJob` reports.
#[derive(Debug, Clone)]
pub struct BackupJob {
    pub job_id: String,
    pub backup_vault_name: String,
    pub backup_vault_arn: String,
    pub recovery_point_arn: String,
    pub resource_arn: String,
    pub resource_type: Option<String>,
    pub iam_role_arn: String,
    pub state: String,
    pub status_message: Option<String>,
    pub percent_done: String,
    /// The emulator copies no bytes, so a job it ran moved nothing: this stays
    /// 0 rather than reporting a size no backup produced.
    pub backup_size_in_bytes: i64,
    pub bytes_transferred: i64,
    pub creation_date: SystemTime,
    /// When the clock owes this job its `CREATED` -> `COMPLETED` transition.
    pub settles_at: SystemTime,
    pub completion_date: Option<SystemTime>,
    /// The deadline `StartWindowMinutes` sets, when the caller gave one.
    pub start_by: Option<SystemTime>,
    pub lifecycle: Option<Lifecycle>,
    pub backup_options: BTreeMap<String, String>,
    pub encryption_key_arn: Option<String>,
    pub is_encrypted: bool,
    pub is_parent: bool,
    pub idempotency_token: Option<String>,
}

/// What a recorded `IdempotencyToken` replays, and when it was recorded so it
/// can be forgotten after [`IDEMPOTENCY_TOKEN_TTL`].
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub job_id: String,
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-backup-cryocontrollerusermanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBackupCryocontrollerusermanagerState {
    /// Vaults by `BackupVaultName`. Bounded by [`MAX_VAULTS`].
    pub vaults: BTreeMap<String, BackupVault>,
    /// Recovery points by `RecoveryPointArn`. Bounded by [`MAX_RECOVERY_POINTS`].
    pub recovery_points: BTreeMap<String, RecoveryPoint>,
    /// Per-vault recovery-point ARNs in insertion order — the single index both
    /// the list operation and `NumberOfRecoveryPoints` read, so it holds exactly
    /// the ARNs `recovery_points` does.
    pub recovery_points_by_vault: BTreeMap<String, Vec<String>>,
    /// Backup jobs by `BackupJobId`. Bounded by [`MAX_BACKUP_JOBS`].
    pub backup_jobs: BTreeMap<String, BackupJob>,
    /// `StartBackupJob`'s `IdempotencyToken` -> what it replays. Bounded by
    /// [`MAX_IDEMPOTENCY_TOKENS`] and [`IDEMPOTENCY_TOKEN_TTL`].
    pub idempotency_tokens: BTreeMap<String, IdempotencyRecord>,
    /// Monotonic counter behind [`Self::next_id`]; never reused within a run.
    id_seq: u64,
}

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// A fresh, unique id in the hyphenated 8-4-4-4-12 hex form AWS Backup job
    /// and recovery-point ids carry.
    ///
    /// Uniqueness comes from `id_seq`, a per-engine counter, so two ids minted
    /// within one nanosecond still differ; the creation instant supplies the
    /// high bits so ids from separate runs do not collide.
    pub fn next_id(&mut self, now: SystemTime) -> String {
        self.id_seq = self.id_seq.wrapping_add(1);
        let nanos = u64::try_from(
            now.duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
        let seq = self.id_seq;
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (nanos >> 32) as u32,
            ((nanos >> 16) & 0xffff) as u16,
            (nanos & 0xffff) as u16,
            ((seq >> 48) & 0xffff) as u16,
            seq & 0xffff_ffff_ffff,
        )
    }

    /// How many recovery points the named vault holds, derived from the index.
    pub fn recovery_point_count(&self, vault_name: &str) -> i64 {
        self.recovery_points_by_vault
            .get(vault_name)
            .map_or(0, |arns| i64::try_from(arns.len()).unwrap_or(i64::MAX))
    }

    /// Materialise the transition the clock has already made due, for ONE job.
    ///
    /// This is the ONLY write a reporting operation performs: a job whose
    /// `settles_at` has passed was owed its completion at that instant whether
    /// or not anybody called, so a describe settles it before reporting.
    /// Nothing else about the job changes, and the completion instant recorded
    /// is the instant it came due — not the instant somebody happened to look.
    /// Only the job asked about is touched, so the cost of a read does not grow
    /// with the store.
    pub fn settle_job(&mut self, job_id: &str, now: SystemTime) {
        if let Some(job) = self.backup_jobs.get_mut(job_id) {
            if job.state == BACKUP_JOB_STATE_CREATED && now >= job.settles_at {
                job.state = BACKUP_JOB_STATE_COMPLETED.to_string();
                job.completion_date = Some(job.settles_at);
                job.percent_done = PERCENT_DONE_COMPLETE.to_string();
                job.bytes_transferred = job.backup_size_in_bytes;
            }
        }
    }

    /// The same, for the recovery points of ONE vault: the `CREATING` ->
    /// `COMPLETED` transition, and the `PENDING` -> `ACTIVE` one for a point
    /// whose job was asked to build an index.
    pub fn settle_recovery_points_in(&mut self, vault_name: &str, now: SystemTime) {
        let arns = self
            .recovery_points_by_vault
            .get(vault_name)
            .cloned()
            .unwrap_or_default();
        for arn in arns {
            let Some(point) = self.recovery_points.get_mut(&arn) else {
                continue;
            };
            if now < point.settles_at {
                continue;
            }
            if point.status == RECOVERY_POINT_STATUS_CREATING {
                point.status = RECOVERY_POINT_STATUS_COMPLETED.to_string();
                point.completion_date = Some(point.settles_at);
            }
            if point.index_status.as_deref() == Some(INDEX_STATUS_PENDING) {
                point.index_status = Some(INDEX_STATUS_ACTIVE.to_string());
            }
        }
    }

    /// Forget every `IdempotencyToken` record past [`IDEMPOTENCY_TOKEN_TTL`],
    /// then the oldest records beyond [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// This is the one store whose KEYS the caller chooses, so it is pruned on
    /// every `StartBackupJob` rather than left to grow with the request count.
    pub fn prune_idempotency_tokens(&mut self, now: SystemTime) {
        self.idempotency_tokens.retain(|_, record| {
            record
                .recorded_at
                .checked_add(IDEMPOTENCY_TOKEN_TTL)
                .is_none_or(|expires_at| now < expires_at)
        });
        while self.idempotency_tokens.len() > MAX_IDEMPOTENCY_TOKENS {
            let oldest = self
                .idempotency_tokens
                .iter()
                .min_by_key(|(_, record)| record.recorded_at)
                .map(|(token, _)| token.clone());
            let Some(oldest) = oldest else { break };
            self.idempotency_tokens.remove(&oldest);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBackupCryocontrollerusermanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBackupCryocontrollerusermanagerState>)
}
