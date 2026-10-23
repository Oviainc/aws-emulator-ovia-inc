// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-backup-cryocontrollerusermanager service.
//!
//! The vault / backup-job slice of AWS Backup is modelled here as three records
//! plus one index, so every operation computes its answer from the SAME stored
//! resource:
//!
//! * [`BackupVault`] — what `CreateBackupVault` inserts and `DeleteBackupVault`
//!   removes, carrying the [`VaultLock`] `PutBackupVaultLockConfiguration`
//!   applies to it.
//! * [`RecoveryPoint`] — what `StartBackupJob` stores inside the named vault.
//! * [`BackupJob`] — the job that produced that recovery point.
//!
//! `recovery_points_by_vault` is the ONLY per-vault list: both
//! `ListRecoveryPointsByBackupVault`'s page and `DescribeBackupVault`'s
//! `NumberOfRecoveryPoints` are derived from it, so the two can never disagree.
//! [`VaultView`] is likewise the ONLY vault read projection: `DescribeBackupVault`
//! and `ListBackupVaults` both answer from it.
//!
//! [`ComAmazonawsBackupCryocontrollerusermanagerState::visible_vault`] (and its
//! `_mut` twin, for a call that writes the vault it resolves) is the resolution
//! step every vault-scoped call AWS Backup authorizes against the named vault
//! shares — the vault READS and the vault-LOCK actions: it applies the access
//! decision before the existence lookup, so none of them can disclose a vault the
//! caller cannot see.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AlreadyExistsException, InvalidParameterValueException, Lifecycle, ResourceNotFoundException,
};

/// The ARN namespace this service stamps into every ARN it mints
/// (`OPERATIONS.md`: "ARN namespace: `backup`").
const ARN_NAMESPACE: &str = "backup";

/// The region an ARN carries when the request is unsigned and therefore names
/// no SigV4 credential scope to read a region from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `VaultType` for an ordinary backup vault (model enum `VaultType`).
pub const VAULT_TYPE_BACKUP_VAULT: &str = "BACKUP_VAULT";
/// `VaultType` for a logically air-gapped backup vault (model enum `VaultType`).
pub const VAULT_TYPE_LOGICALLY_AIR_GAPPED_BACKUP_VAULT: &str =
    "LOGICALLY_AIR_GAPPED_BACKUP_VAULT";
/// `VaultState` of a vault that is ready to receive backups (model enum `VaultState`).
pub const VAULT_STATE_AVAILABLE: &str = "AVAILABLE";
/// `VaultState` of a vault whose creation has not settled yet (model enum
/// `VaultState`).
pub const VAULT_STATE_CREATING: &str = "CREATING";
/// `EncryptionKeyType` when the caller supplied no `EncryptionKeyArn`.
pub const ENCRYPTION_KEY_TYPE_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";
/// `EncryptionKeyType` when the caller supplied an `EncryptionKeyArn`.
pub const ENCRYPTION_KEY_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// The ARN namespace of the KMS keys a vault is encrypted with.
const KMS_ARN_NAMESPACE: &str = "kms";

/// The least `ChangeableForDays` accepts, as the model documents it: "Backup
/// enforces a 72-hour cooling-off period before Vault Lock takes effect and
/// becomes immutable. Therefore, you must set `ChangeableForDays` to 3 or
/// greater."
pub const MIN_CHANGEABLE_FOR_DAYS: i64 = 3;

/// The most `ChangeableForDays` accepts, as the model documents it: "The maximum
/// value you can specify is 36,500 days (approximately 100 years)."
pub const MAX_CHANGEABLE_FOR_DAYS: i64 = 36_500;

/// The least a vault-lock retention bound accepts, as the model documents for
/// `MinRetentionDays`: "The shortest minimum retention period you can specify is
/// 1 day."
pub const MIN_LOCK_RETENTION_DAYS: i64 = 1;

/// The most a vault-lock retention bound accepts, as the model documents for
/// `MaxRetentionDays`: "The longest maximum retention period you can specify is
/// 36500 days (approximately 100 years)."
pub const MAX_LOCK_RETENTION_DAYS: i64 = 36_500;

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

/// The smallest `MaxResults` this service accepts: the minimum of the model's
/// `@range` on the `MaxResults` shape (`smithy_query` operation
/// `ListProtectedResources`: `MaxResults` `range {min: 1, max: 1000}`), which
/// every paginated Backup operation's `MaxResults` targets.
pub const MAX_RESULTS_MIN: i32 = 1;
/// The largest `MaxResults` this service accepts: the maximum of that same
/// `@range`.
pub const MAX_RESULTS_MAX: i32 = 1000;

/// The page size a list operation uses when the caller names no `MaxResults`:
/// the model's own `MaxResults` maximum, so one unpaginated call returns
/// everything a paginated one could.
pub const MAX_PAGE_SIZE: usize = MAX_RESULTS_MAX as usize;

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
/// ceiling: the key is a string the CALLER picks, so this is one of the two
/// stores whose keys a caller controls (the other is
/// [`MAX_DEFAULT_ENCRYPTION_KEYS`]'s). Past the ceiling the oldest record is
/// dropped — a dropped token simply stops replaying, which is the same answer a
/// caller gets for a token it never sent.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1_000;

/// The most default encryption keys this emulator keeps: one per region the
/// requests it has served have named. An EMULATOR-CHOSEN ceiling for the reason
/// given on [`MAX_VAULTS`] — the region comes from the caller's credential
/// scope, so this store's keys are caller-chosen too. Past the ceiling another
/// region's key is dropped; every vault keeps the key ARN recorded ON it, so a
/// drop only means the next vault created in that region is encrypted by a
/// newly created default key.
pub const MAX_DEFAULT_ENCRYPTION_KEYS: usize = 64;

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

/// How long after a create the vault settles from `CREATING` to `AVAILABLE`.
///
/// A CHOSEN span, not a documented one: AWS takes as long as provisioning the
/// vault needs. It is deliberately non-zero so a caller that describes or
/// deletes the vault straight after creating it still observes the transient
/// `CREATING` state and the refusal that goes with it, and the due instant is
/// stored on the vault ([`BackupVault::settles_at`]) so no handler ever sleeps
/// for it.
pub const VAULT_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The smallest `MinRetentionDays` a logically air-gapped vault accepts, as the
/// model documents for the member ("This setting specifies the minimum
/// retention period that the vault retains its recovery points. The minimum
/// value accepted is 7 days").
pub const MIN_RETENTION_DAYS_FLOOR: i64 = 7;

/// The `Code` AWS Backup carries on the `InvalidRequestException` that refuses
/// an operation on a vault still in `CREATING` state.
pub const ERROR_CODE_VAULT_STILL_CREATING: &str = "ERROR_9100";

/// A seconds-per-day constant for turning `Lifecycle` day counts into the
/// instants `CalculatedLifecycle` reports.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The ARN of a backup vault: `arn:aws:backup:<region>:<account>:backup-vault:<name>`.
pub fn backup_vault_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("backup-vault:{name}"))
}

/// The ARN of a KMS key: `arn:aws:kms:<region>:<account>:key/<key id>`.
pub fn kms_key_arn(region: &str, key_id: &str) -> String {
    ::engine::emulator_arn(KMS_ARN_NAMESPACE, region, &format!("key/{key_id}"))
}

/// The ARN of a recovery point: `arn:aws:backup:<region>:<account>:recovery-point:<id>`.
pub fn recovery_point_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("recovery-point:{id}"))
}

/// The region this request's ARNs carry: the one the caller signed for, from the
/// SigV4 credential scope, falling back to [`DEFAULT_REGION`] for an unsigned
/// request — or for a credential scope whose region field is not shaped like a
/// region name, which names no region this service can mint an ARN for.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `value` is shaped like an AWS Region name: 1 to 32 lowercase
/// letters, digits and hyphens, which is the form every published region name
/// takes (`us-east-1`, `ap-southeast-4`, `us-gov-west-1`).
///
/// The region reaches this service as a free-form field of the caller's SigV4
/// credential scope, so it is checked before it is stitched into an ARN or used
/// as a key of this service's own state.
fn is_region_like(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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

// ===========================================================================
// This service's error vocabulary
// ===========================================================================
//
// Every AWS Backup client-error shape carries `Code`, `Message`, `Type` and
// `Context` members (`smithy_query(query="shape", shape="InvalidParameterValueException")`),
// and a real response fills `Code` with this service's own token for the
// CONDITION — not for the exception class, since one class answers several
// conditions with different tokens. Each condition whose token this emulator
// has observed names it once here and builds its exception once below, so two
// operations can never answer one condition with two codes or two sentences.
//
// A condition with no observed token leaves `Code` unset (the model makes the
// member optional): inventing an `ERROR_nnnn` would hand a client branching on
// the code a token AWS never sends, which is worse than the absent member.
//
// A token below, and a sentence marked FIXED, is one AWS states for the
// condition rather than for the request — it names no part of the request, so
// it is a constant here and not a format string.

/// `Code` for "the backup job asked about is not one this account holds".
pub const ERROR_CODE_BACKUP_JOB_NOT_FOUND: &str = "ERROR_2403";
/// The sentence AWS states for that condition. A FIXED string: it names no part
/// of the request, so the id asked about does not appear in it.
pub const MESSAGE_BACKUP_JOB_NOT_FOUND: &str = "Cannot find Backup job";

/// `Code` for "a backup vault of this name already exists".
pub const ERROR_CODE_VAULT_ALREADY_EXISTS: &str = "ERROR_1101";
/// The sentence AWS states for that condition — fixed, like the one above.
pub const MESSAGE_VAULT_ALREADY_EXISTS: &str = "Backup vault with the same name already exists";

/// `Code` for "`MaxResults` is outside the range the model declares".
pub const ERROR_CODE_MAX_RESULTS_INVALID: &str = "ERROR_3607";
/// The sentence AWS states for that condition — fixed, like the ones above.
pub const MESSAGE_MAX_RESULTS_INVALID: &str = "Max Results is invalid";

/// The values the model's `AggregationPeriod` enum admits, in model order
/// (`smithy_query(query="shape", shape="AggregationPeriod")`). The summary
/// operations name this list in the message they refuse an unknown period with,
/// so the list a caller is told about is the list the model declares.
pub const AGGREGATION_PERIODS: [&str; 3] = ["ONE_DAY", "SEVEN_DAYS", "FOURTEEN_DAYS"];

/// The modelled refusal for an unknown backup job id, with its `Code`.
pub fn backup_job_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        code: Some(ERROR_CODE_BACKUP_JOB_NOT_FOUND.to_string()),
        message: Some(MESSAGE_BACKUP_JOB_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a create naming a vault that already exists.
///
/// `Arn` and `CreatorRequestId` are left UNSET: the shape declares them, but a
/// real response to this condition does not disclose the existing vault's ARN
/// or the token its creator sent.
pub fn vault_already_exists() -> AlreadyExistsException {
    AlreadyExistsException {
        code: Some(ERROR_CODE_VAULT_ALREADY_EXISTS.to_string()),
        message: Some(MESSAGE_VAULT_ALREADY_EXISTS.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a `MaxResults` outside
/// [`MAX_RESULTS_MIN`]`..=`[`MAX_RESULTS_MAX`], or `None` when the caller's
/// page size is one the model admits (including an absent one).
///
/// The bound is the model's own, so an operation calls this instead of spelling
/// the range again. The check belongs to the OPERATION rather than to the
/// framework's boundary validator: AWS answers it with this modelled exception,
/// and only an exception the operation returns is serialized as the shape the
/// model declares — the body a client reads `Code` and `Message` out of.
pub fn rejected_max_results(max_results: Option<i32>) -> Option<InvalidParameterValueException> {
    let value = max_results?;
    if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&value) {
        return None;
    }
    Some(InvalidParameterValueException {
        code: Some(ERROR_CODE_MAX_RESULTS_INVALID.to_string()),
        message: Some(MESSAGE_MAX_RESULTS_INVALID.to_string()),
        ..Default::default()
    })
}

/// The modelled refusal for an `AggregationPeriod` the model's enum does not
/// list, or `None` when the caller named one it does (including none at all).
///
/// The message is BUILT FROM THE REQUEST: it names the rejected value and then
/// the values the model admits, so a caller can fix the request from the
/// response alone. No `Code` is set — this condition has no observed token.
pub fn rejected_aggregation_period(
    aggregation_period: Option<&str>,
) -> Option<InvalidParameterValueException> {
    let value = aggregation_period?;
    if AGGREGATION_PERIODS.contains(&value) {
        return None;
    }
    Some(InvalidParameterValueException {
        message: Some(format!(
            "Invalid input aggregation id {value}. Input aggregation period must be within {}",
            AGGREGATION_PERIODS.join(", ")
        )),
        ..Default::default()
    })
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

/// The Backup Vault Lock configuration applied to a vault by
/// `PutBackupVaultLockConfiguration`.
///
/// Its PRESENCE is what the vault read projections report as `Locked`, so a
/// vault can never report retention bounds it is not locked by, nor a lock with
/// no bounds behind it.
#[derive(Debug, Clone)]
pub struct VaultLock {
    pub min_retention_days: Option<i64>,
    pub max_retention_days: Option<i64>,
    /// When the configuration becomes immutable — set only when the request
    /// named `ChangeableForDays`, since the model documents that "if this
    /// parameter is not specified, you can delete Vault Lock from the vault
    /// [...] or change the Vault Lock configuration [...] at any time".
    pub lock_date: Option<SystemTime>,
}

impl VaultLock {
    /// Whether this configuration can no longer be changed or deleted: the
    /// model documents that "on and after the lock date, the Vault Lock becomes
    /// immutable and cannot be changed or deleted".
    pub fn is_immutable_at(&self, now: SystemTime) -> bool {
        self.lock_date.is_some_and(|lock_date| now >= lock_date)
    }
}

/// The resource types this service supports, which is what
/// `GetSupportedResourceTypes` reports and what every other operation that
/// names a `ResourceType` is measured against.
///
/// SOURCE, for all but the last two entries: the model's own documentation for
/// the member being reported —
/// `smithy_query(query="doc", shape="GetSupportedResourceTypesOutput",
/// member="ResourceTypes", model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")`
/// — which enumerates each supported type with the service it stands for.
///
/// The last two, `DSQL` (Aurora DSQL) and `Redshift Serverless`, are services
/// AWS Backup supports whose types that member's documentation predates; AWS
/// reports them in the list today. Membership is Region- and era-dependent at
/// AWS — a Region that has a newer service adds its type — so the catalog is a
/// set this emulator OWNS: it is what this emulator supports, and an operation
/// that answers for a resource type reads it from here rather than carrying a
/// list of its own.
///
/// Three of the spellings (`Storage Gateway`, `SAP HANA on Amazon EC2`,
/// `Redshift Serverless`) contain spaces, which the `ResourceType` shape's own
/// `@pattern` (`^[a-zA-Z0-9\-\_\.]{1,50}$`) refuses. That pattern governs what
/// a CALLER may send — AWS reports these spellings and refuses them as input
/// just the same — so they are reported here as AWS spells them. See
/// `is_modelled_resource_type`, which is the other side of the same line: a
/// type this service DERIVES for a stored resource is only reported in a form
/// the pattern admits.
pub const SUPPORTED_RESOURCE_TYPES: &[&str] = &[
    "Aurora",
    "CloudFormation",
    "DSQL",
    "DocumentDB",
    "DynamoDB",
    "EBS",
    "EC2",
    "EFS",
    "EKS",
    "FSx",
    "Neptune",
    "RDS",
    "Redshift",
    "Redshift Serverless",
    "S3",
    "SAP HANA on Amazon EC2",
    "Storage Gateway",
    "Timestream",
    "VirtualMachine",
];

/// A backup vault: the logical container `CreateBackupVault` inserts.
#[derive(Debug, Clone)]
pub struct BackupVault {
    pub name: String,
    pub arn: String,
    /// The KMS key this vault is encrypted with. ALWAYS set: the model
    /// documents that "if you do not specify a key, Backup creates a KMS key
    /// for you by default", so a vault always has one to report.
    pub encryption_key_arn: String,
    /// Whether [`Self::encryption_key_arn`] is the key the create request
    /// named, rather than the one this service created for the account by
    /// default ([`ComAmazonawsBackupCryocontrollerusermanagerState::default_encryption_key_arn`]).
    pub customer_managed_key: bool,
    /// `EncryptionKeyType`, as this vault's type reports it. `None` for a
    /// standard `BACKUP_VAULT`: AWS leaves the member unset for one, and a read
    /// projects what is stored rather than defaulting an enum nobody set.
    pub encryption_key_type: Option<String>,
    pub creator_request_id: Option<String>,
    pub creation_date: SystemTime,
    /// [`VAULT_TYPE_BACKUP_VAULT`] for a vault made by `CreateBackupVault` and
    /// [`VAULT_TYPE_LOGICALLY_AIR_GAPPED_BACKUP_VAULT`] for one made by
    /// `CreateLogicallyAirGappedBackupVault`; `RESTORE_ACCESS_BACKUP_VAULT`
    /// belongs to an operation still at 501.
    pub vault_type: String,
    /// `VaultState`, as this vault's type reports it. `None` for a standard
    /// `BACKUP_VAULT`, for the reason given on [`Self::encryption_key_type`];
    /// the vault types that do carry a state are created by operations still at
    /// 501.
    pub vault_state: Option<String>,
    /// When the clock owes this vault its `CREATING` -> `AVAILABLE`
    /// transition, for a vault whose create reports a non-terminal state. See
    /// [`VAULT_SETTLE_AFTER`]. `None` for a vault created ready.
    pub settles_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// The vault-lock configuration, once one has been applied.
    pub lock: Option<VaultLock>,
}

impl BackupVault {
    /// The `EncryptionKeyArn` the create request NAMED, as opposed to the key
    /// this service created for a request that named none — what a repeat
    /// create is compared against to decide whether it replays the original.
    pub fn named_encryption_key_arn(&self) -> Option<&str> {
        self.customer_managed_key
            .then_some(self.encryption_key_arn.as_str())
    }
}

/// The members a vault read projects, computed ONCE from the stored vault so
/// `DescribeBackupVault` and `ListBackupVaults` can never disagree about the
/// same vault. A member the stored vault does not carry stays `None` here, and
/// so is absent from both responses.
#[derive(Debug, Clone)]
pub struct VaultView {
    pub name: String,
    pub arn: String,
    pub vault_type: String,
    pub vault_state: Option<String>,
    pub encryption_key_arn: String,
    pub encryption_key_type: Option<String>,
    pub creation_date: SystemTime,
    pub creator_request_id: Option<String>,
    pub number_of_recovery_points: i64,
    pub locked: bool,
    pub min_retention_days: Option<i64>,
    pub max_retention_days: Option<i64>,
    pub lock_date: Option<SystemTime>,
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
    /// The KMS key this service created for the account per region, for vaults
    /// whose create request named none. One key per region, minted on first
    /// use, so every default-key vault in a region reports the same key on
    /// every read. Bounded by [`MAX_DEFAULT_ENCRYPTION_KEYS`].
    pub default_encryption_keys: BTreeMap<String, String>,
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

    /// Materialise the transition the clock has already made due, for ONE
    /// vault: the `CREATING` -> `AVAILABLE` transition a create that reports a
    /// non-terminal state owes. Like [`Self::settle_job`] this is the only
    /// write a reporting operation performs — the vault was owed its
    /// availability at `settles_at` whether or not anybody called — and only
    /// the vault asked about is touched.
    pub fn settle_vault(&mut self, vault_name: &str, now: SystemTime) {
        if let Some(vault) = self.vaults.get_mut(vault_name) {
            if vault.vault_state.as_deref() == Some(VAULT_STATE_CREATING)
                && vault.settles_at.is_some_and(|due| now >= due)
            {
                vault.vault_state = Some(VAULT_STATE_AVAILABLE.to_string());
            }
        }
    }

    /// The same for every stored vault, for an operation that reports them all:
    /// a list and a describe of the same vault can then never disagree about
    /// its state.
    pub fn settle_vaults(&mut self, now: SystemTime) {
        let names: Vec<String> = self
            .vaults
            .iter()
            .filter(|(_, vault)| vault.vault_state.as_deref() == Some(VAULT_STATE_CREATING))
            .map(|(name, _)| name.clone())
            .collect();
        for name in names {
            self.settle_vault(&name, now);
        }
    }

    /// The KMS key this service encrypts a vault with when the create request
    /// named none: "if you do not specify a key, Backup creates a KMS key for
    /// you by default" (model doc for `EncryptionKeyArn`).
    ///
    /// Created on first use and then kept, so it is one key per region for the
    /// account — stable across reads and across vaults, as an account's default
    /// key is. The store is bounded by [`MAX_DEFAULT_ENCRYPTION_KEYS`]: the
    /// region is a field of the caller's credential scope, so the keys of this
    /// map are caller-chosen and it is pruned rather than left to grow.
    pub fn default_encryption_key_arn(&mut self, region: &str, now: SystemTime) -> String {
        if let Some(arn) = self.default_encryption_keys.get(region) {
            return arn.clone();
        }
        let arn = kms_key_arn(region, &self.next_id(now));
        self.default_encryption_keys
            .insert(region.to_string(), arn.clone());
        while self.default_encryption_keys.len() > MAX_DEFAULT_ENCRYPTION_KEYS {
            // Never the key just created: it is the one this request answers
            // with, and the vault being created records it.
            let evictable = self
                .default_encryption_keys
                .keys()
                .find(|key| key.as_str() != region)
                .cloned();
            let Some(evictable) = evictable else { break };
            self.default_encryption_keys.remove(&evictable);
        }
        arn
    }

    /// Project one stored vault for a read. The ONE place a vault's wire view
    /// is computed; both vault read operations go through it.
    pub fn vault_view(&self, vault: &BackupVault) -> VaultView {
        VaultView {
            name: vault.name.clone(),
            arn: vault.arn.clone(),
            vault_type: vault.vault_type.clone(),
            vault_state: vault.vault_state.clone(),
            encryption_key_arn: vault.encryption_key_arn.clone(),
            encryption_key_type: vault.encryption_key_type.clone(),
            creation_date: vault.creation_date,
            creator_request_id: vault.creator_request_id.clone(),
            number_of_recovery_points: self.recovery_point_count(&vault.name),
            // A vault reports itself locked exactly while it holds a vault-lock
            // configuration, and reports that configuration's bounds.
            locked: vault.lock.is_some(),
            min_retention_days: vault.lock.as_ref().and_then(|lock| lock.min_retention_days),
            max_retention_days: vault.lock.as_ref().and_then(|lock| lock.max_retention_days),
            lock_date: vault.lock.as_ref().and_then(|lock| lock.lock_date),
        }
    }

    /// How many recovery points the named vault holds, derived from the index.
    pub fn recovery_point_count(&self, vault_name: &str) -> i64 {
        self.recovery_points_by_vault
            .get(vault_name)
            .map_or(0, |arns| i64::try_from(arns.len()).unwrap_or(i64::MAX))
    }

    /// Resolve a vault for a vault-scoped READ, deciding access before existence.
    ///
    /// AWS Backup authorizes a vault-scoped read against the named vault as the
    /// resource, so a caller that may not see that vault is refused for lack of
    /// privileges and told nothing about whether it exists. A vault is outside the
    /// caller's visibility when the account the request is scoped to does not hold it
    /// — it was never created, or it has been deleted — or when the request addresses
    /// another account's vault with `BackupVaultAccountId`; this emulator holds
    /// exactly one account's vaults ([`engine::emulator_account_id`]), so any other
    /// account id is another account's.
    ///
    /// Reporting only `Err(AccessDeniedException)` is the point: a caller cannot ask
    /// this step whether an invisible vault exists, so no read can leak it.
    pub fn visible_vault(
        &self,
        vault_name: &str,
        vault_account_id: Option<&str>,
    ) -> Result<&BackupVault, crate::scaffolded::AccessDeniedException> {
        let own_account = Self::is_own_account(vault_account_id);
        self.vaults
            .get(vault_name)
            .filter(|_| own_account)
            .ok_or_else(crate::scaffolded::AccessDeniedException::insufficient_privileges)
    }

    /// The SAME decision for a vault-scoped call that WRITES the vault it resolves.
    ///
    /// AWS Backup authorizes a vault-lock action (`PutBackupVaultLockConfiguration`,
    /// `DeleteBackupVaultLockConfiguration`) against the named vault as the resource,
    /// so a vault-lock call naming a vault outside the caller's visibility is refused
    /// for lack of privileges and told nothing about whether that vault exists —
    /// exactly as a vault-scoped read is. Both paths therefore share ONE decision
    /// rather than each deciding for itself.
    ///
    /// The vault-lock inputs model no `BackupVaultAccountId` (`smithy_query(
    /// query="operation", operation="PutBackupVaultLockConfiguration")` lists
    /// `BackupVaultName` plus the three day counts only), so those handlers pass
    /// `None`; the parameter is kept so that every vault-scoped resolution, read or
    /// write, states the account it is scoped to.
    pub fn visible_vault_mut(
        &mut self,
        vault_name: &str,
        vault_account_id: Option<&str>,
    ) -> Result<&mut BackupVault, crate::scaffolded::AccessDeniedException> {
        let own_account = Self::is_own_account(vault_account_id);
        self.vaults
            .get_mut(vault_name)
            .filter(|_| own_account)
            .ok_or_else(crate::scaffolded::AccessDeniedException::insufficient_privileges)
    }

    /// Whether a `BackupVaultAccountId` names the one account whose vaults this
    /// emulator holds ([`engine::emulator_account_id`]); `None` is the caller's own
    /// account, so it always does.
    fn is_own_account(vault_account_id: Option<&str>) -> bool {
        vault_account_id.is_none_or(|account| account == ::engine::emulator_account_id())
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
    /// This is one of the two stores whose KEYS the caller chooses, so it is
    /// pruned on
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
