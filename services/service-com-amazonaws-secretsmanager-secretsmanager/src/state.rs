// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-secretsmanager-secretsmanager service.
//!
//! The shape of the resource this service stores: a secret is identified by its
//! name, carries an ARN that ends in the name plus a hyphen and six random
//! characters, and owns an insertion-ordered list of versions. The staging
//! labels (`AWSCURRENT`, `AWSPREVIOUS`, and any label a caller names) live on
//! the versions, and a label is attached to at most one version of a secret at a
//! time; a version that carries no label is deprecated and is left out of
//! `VersionIdsToStages` / `SecretVersionsToStages`.
//!
//! A secret also carries the rotation configuration `RotateSecret` set for it —
//! the rotation function, the rules, the external-rotation metadata and the
//! instant the schedule next makes a rotation due — and, while a rotation it
//! started has not finished, the version id that rotation staked `AWSPENDING`
//! on before its rotation function created that version.
//!
//! Everything a handler needs to resolve, mutate or project a secret is a method
//! here, so the per-operation files hold only that operation's own rules.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ExternalSecretRotationMetadataItem, ReplicationStatusType, RotationRulesType, Tag,
};

/// The staging label on the current value of a secret.
pub const STAGE_CURRENT: &str = "AWSCURRENT";

/// The staging label that moves to the version `AWSCURRENT` was taken from.
pub const STAGE_PREVIOUS: &str = "AWSPREVIOUS";

/// The staging label a rotation stages the new secret value under: "AWSPENDING,
/// which indicates the version of the secret that contains new secret
/// information that will become the next current version when rotation
/// finishes" (`DescribeSecretResponse$VersionIdsToStages`, this service's
/// Smithy model).
pub const STAGE_PENDING: &str = "AWSPENDING";

/// Secrets Manager appends "a hyphen and six random characters after the secret
/// name at the end of the ARN" (`CreateSecretRequest$Name`, this service's
/// Smithy model).
pub const ARN_SUFFIX_LEN: usize = 6;

/// The recovery window `DeleteSecret` applies when the caller names none: "If
/// you don't use either, then by default Secrets Manager uses a 30 day recovery
/// window." (`DeleteSecretRequest$RecoveryWindowInDays`, this service's Smithy
/// model.)
pub const DEFAULT_RECOVERY_WINDOW_DAYS: i64 = 30;

/// "The number of days from 7 to 30 that Secrets Manager waits before
/// permanently deleting the secret." (`DeleteSecretRequest$RecoveryWindowInDays`,
/// this service's Smithy model.)
pub const MIN_RECOVERY_WINDOW_DAYS: i64 = 7;

/// The upper bound of the same documented range.
pub const MAX_RECOVERY_WINDOW_DAYS: i64 = 30;

/// How long a `ForceDeleteWithoutRecovery` deletion, which skips the recovery
/// window, stays marked for deletion before the store owes its permanent
/// removal.
///
/// "Secrets Manager performs the actual deletion with an asynchronous background
/// process, so there might be a short delay before the secret is permanently
/// deleted." (`DeleteSecretRequest$ForceDeleteWithoutRecovery`, this service's
/// Smithy model.) AWS documents no duration for that delay, so this span is the
/// emulator's OWN: long enough that a caller acting straight after the call
/// still observes the secret marked for deletion — and is refused by its state
/// rather than told it does not exist — short enough that a client retrying a
/// create of the same name does not wait on a human timescale.
pub const FORCE_DELETE_PURGE_DELAY: Duration = Duration::from_millis(500);

/// The message `ResourceNotFoundException` carries when a `SecretId` names no
/// secret this service holds.
///
/// `ResourceNotFoundException` is documented as "Secrets Manager can't find the
/// resource that you asked for." (this service's Smithy model), and the wire
/// message Secrets Manager answers with is this one constant sentence: it names
/// no identifier, so the request's `SecretId` is never reflected back into the
/// error body. Every operation that resolves a `SecretId` answers with THIS
/// constant, so one secret-not-found error reads the same whichever operation
/// raised it.
pub const SECRET_NOT_FOUND_MESSAGE: &str = "Secrets Manager can't find the specified secret.";

/// The message `InvalidNextTokenException` carries for a pagination token this
/// service cannot resume from.
///
/// `InvalidNextTokenException` is documented as "The `NextToken` value is
/// invalid." (this service's Smithy model); the wire message is this constant
/// sentence, and it likewise carries no part of the request. Every paginated
/// operation answers with THIS constant.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The pagination token is no longer valid.";

/// The message `InvalidRequestException` carries for an operation a caller
/// cannot perform on a secret that is inside its recovery window: the secret
/// still exists ("The secret is scheduled for deletion.",
/// `InvalidRequestException`, this service's Smithy model), so it is refused by
/// state rather than reported as missing.
pub const MARKED_FOR_DELETION_MESSAGE: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// The `InvalidParameterException` message for a `RecoveryWindowInDays` outside
/// the documented range, stating that constraint: "The number of days from 7 to
/// 30 that Secrets Manager waits before permanently deleting the secret."
/// (`DeleteSecretRequest$RecoveryWindowInDays`, this service's Smithy model).
/// The bounds come from the same constants the validation enforces, and the
/// rejected value is not echoed back.
pub fn recovery_window_out_of_range_message() -> String {
    format!(
        "The RecoveryWindowInDays value must be between {MIN_RECOVERY_WINDOW_DAYS} and \
         {MAX_RECOVERY_WINDOW_DAYS} days (inclusive)."
    )
}

/// Seconds in a day, for turning a recovery window into an instant.
const SECONDS_PER_DAY: u64 = 86_400;

/// Seconds in an hour, for turning a `rate(n hours)` rotation interval into a
/// duration.
const SECONDS_PER_HOUR: u64 = 3_600;

/// The shortest rotation interval a schedule expression may ask for: "You can
/// rotate a secret as often as every four hours"
/// (`RotationRulesType$ScheduleExpression`, this service's Smithy model).
pub const MIN_ROTATION_INTERVAL_HOURS: u64 = 4;

/// The longest rotation interval a schedule may ask for, in days.
///
/// The EMULATOR's own bound, not a documented AWS one: an interval has to be
/// addable to an instant, so a magnitude that cannot be represented is refused
/// where the caller is told why rather than carried into instant arithmetic. Its
/// value is the upper bound of the range the model puts on the other way of
/// naming the same interval, `AutomaticallyAfterDays`
/// (`AutomaticallyRotateAfterDaysType`, range 1..1000, this service's Smithy
/// model), so the two ways of asking for one interval accept the same span.
pub const MAX_ROTATION_INTERVAL_DAYS: u64 = 1_000;

/// The units a `rate()` rotation interval is expressed in, and the seconds each
/// one counts: "Secrets Manager rate() expressions represent the interval in
/// hours or days that you want to rotate your secret, for example rate(12
/// hours) or rate(10 days)" (`RotationRulesType$ScheduleExpression`, this
/// service's Smithy model). Both the singular and the plural are accepted, as
/// the documented examples use the plural and a rate of one is singular.
const RATE_UNITS: &[(&str, u64)] = &[
    ("hour", SECONDS_PER_HOUR),
    ("hours", SECONDS_PER_HOUR),
    ("day", SECONDS_PER_DAY),
    ("days", SECONDS_PER_DAY),
];

/// How many fields the documented cron form carries: "A cron() or rate()
/// expression that defines the schedule for rotating your secret", in the
/// six-field form `cron(minutes hours day-of-month month day-of-week year)`
/// (`RotationRulesType$ScheduleExpression`, this service's Smithy model).
const CRON_FIELDS: usize = 6;

/// What a schedule expression that is not one of the two documented forms is
/// refused with.
pub const SCHEDULE_EXPRESSION_SYNTAX: &str = "ScheduleExpression must be a rate() or cron() \
     expression, for example rate(10 days) or cron(0 8 1 * ? *).";

/// The interval the schedule in `rules` rotates the secret on, or the
/// `InvalidParameterException` message for rules that cannot hold together.
///
/// `Ok(None)` is rules this emulator computes no next due instant from: rules
/// that name no schedule at all (a bare `Duration`, or an empty structure), and
/// a `cron()` expression — see [`parse_schedule_expression`].
pub fn parse_rotation_rules(rules: &RotationRulesType) -> Result<Option<Duration>, String> {
    // "In RotateSecret, you can set the rotation schedule in RotationRules with
    // AutomaticallyAfterDays or ScheduleExpression, but not both."
    if rules.automatically_after_days.is_some() && rules.schedule_expression.is_some() {
        return Err("You can specify either AutomaticallyAfterDays or ScheduleExpression in \
                    RotationRules, but not both."
            .to_string());
    }
    if let Some(days) = rules.automatically_after_days {
        return Ok(Some(interval_of(
            u64::try_from(days).unwrap_or_default(),
            SECONDS_PER_DAY,
            "AutomaticallyAfterDays",
        )?));
    }
    match rules.schedule_expression.as_deref() {
        Some(expression) => parse_schedule_expression(expression),
        None => Ok(None),
    }
}

/// The interval a `ScheduleExpression` names.
///
/// A `rate()` expression names one outright. A `cron()` expression names
/// occurrences on a calendar rather than an interval, so there is nothing to add
/// to an instant: it is checked for the six fields the service documents, stored
/// and reported back unchanged, and answers `Ok(None)` — this emulator reports
/// `NextRotationDate` for the interval forms and OMITS it for a `cron()`
/// schedule rather than reporting an instant it did not compute.
fn parse_schedule_expression(expression: &str) -> Result<Option<Duration>, String> {
    let expression = expression.trim();
    let body = |keyword: &str| {
        expression
            .strip_prefix(keyword)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.strip_suffix(')'))
    };
    if let Some(rate) = body("rate") {
        let mut parts = rate.split_whitespace();
        let (Some(value), Some(unit), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(SCHEDULE_EXPRESSION_SYNTAX.to_string());
        };
        let value: u64 = value
            .parse()
            .map_err(|_| SCHEDULE_EXPRESSION_SYNTAX.to_string())?;
        let seconds = RATE_UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, seconds)| *seconds)
            .ok_or_else(|| SCHEDULE_EXPRESSION_SYNTAX.to_string())?;
        return Ok(Some(interval_of(value, seconds, "ScheduleExpression")?));
    }
    if let Some(fields) = body("cron") {
        let fields = fields.split_whitespace().count();
        if fields != CRON_FIELDS {
            return Err(format!(
                "ScheduleExpression must carry a cron expression of {CRON_FIELDS} fields \
                 (minutes hours day-of-month month day-of-week year), not {fields}."
            ));
        }
        return Ok(None);
    }
    Err(SCHEDULE_EXPRESSION_SYNTAX.to_string())
}

/// `value` units of `seconds` as a rotation interval, refused when it falls
/// outside the interval the service's own rules allow: no more often than every
/// four hours, and no longer than the maximum `AutomaticallyAfterDays` names.
///
/// `member` is the member the refusal names, so the caller is told which half of
/// `RotationRules` it has to fix.
fn interval_of(value: u64, seconds: u64, member: &str) -> Result<Duration, String> {
    let total = value.saturating_mul(seconds);
    if total < MIN_ROTATION_INTERVAL_HOURS * SECONDS_PER_HOUR {
        return Err(format!(
            "{member} must schedule a rotation no more often than every \
             {MIN_ROTATION_INTERVAL_HOURS} hours."
        ));
    }
    if total > MAX_ROTATION_INTERVAL_DAYS * SECONDS_PER_DAY {
        return Err(format!(
            "{member} must not schedule a rotation further apart than every \
             {MAX_ROTATION_INTERVAL_DAYS} days."
        ));
    }
    Ok(Duration::from_secs(total))
}

/// How many secrets the store holds before `CreateSecret` refuses with the
/// modelled `LimitExceededException`.
///
/// Secrets Manager publishes an adjustable per-Region secret quota, and this
/// emulator models neither Service Quotas nor an account's adjustments — so this
/// is the emulator's OWN bound on the store, not a claim about any account's
/// limit. It bounds the WHOLE store rather than one region's share of it: the
/// region a secret is homed in comes from the request, so a per-region bound
/// would still let a caller grow the store without end by varying the region it
/// signs for. Every record it holds is released by `DeleteSecret`, and it sits
/// far above any workflow an emulator serves.
pub const SECRET_QUOTA: usize = 1_000;

/// How many versions one secret holds before a further `PutSecretValue` has to
/// make room by evicting a deprecated one.
///
/// Secrets Manager removes a version once it carries no staging label, so
/// nothing a caller can reach by stage depends on a deprecated version
/// surviving; keeping a bounded tail of them is what lets a deprecated
/// `VersionId` still be read for a while, as the real service does. The bound is
/// the emulator's own, and it is what stops a loop of `PutSecretValue` calls
/// against one secret from growing the store without end.
pub const VERSIONS_PER_SECRET: usize = 100;

/// The region a resource is placed in when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many tags one secret carries before a further `TagResource` is refused.
///
/// The AWS tagging quota is 50 tags per resource ("Service quotas for Tagging",
/// Amazon Web Services General Reference, referenced from `TagResource`'s own
/// documentation in this service's Smithy model). It is what stops a loop of
/// `TagResource` calls against one secret from growing the store without end.
pub const TAGS_PER_SECRET: usize = 50;

/// The prefix AWS reserves for its own tags: a caller may neither create nor
/// modify a tag whose key starts with it ("Service quotas for Tagging", Amazon
/// Web Services General Reference).
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// How many replica Regions one secret holds before a further replication
/// request is refused, and the longest `AddReplicaRegions` list a request may
/// carry.
///
/// A replica lives in an AWS Region, and the set of Regions is finite and
/// published — but `RegionType` is only a length-bounded pattern
/// (`^([a-z]+-)+\d+$`), and this emulator validates no Region catalogue, so a
/// caller could otherwise mint unbounded distinct Region strings and grow one
/// secret's replica list without end. This bound is therefore the emulator's
/// OWN, chosen above the number of Regions AWS publishes, and it is what keeps
/// the resident state (and the per-request walk in [`settle_replicas`]) finite.
/// `AddReplicaRegionListType` declares no maximum length, so the request's own
/// list is bounded by the same constant before anything is counted or stored.
///
/// [`settle_replicas`]: Secret::settle_replicas
pub const REPLICAS_PER_SECRET: usize = 64;

/// The KMS key a replica is encrypted with when the request names none: "The
/// ARN, key ID, or alias of the KMS key to encrypt the secret. If you don't
/// include this field, Secrets Manager uses `aws/secretsmanager`."
/// (`ReplicaRegionType$KmsKeyId`, this service's Smithy model.) The alias form
/// is what the service reports back in a `ReplicationStatusType`.
pub const DEFAULT_KMS_KEY_ALIAS: &str = "alias/aws/secretsmanager";

/// `ReplicationStatusType$Status` while the replica is being created
/// (`StatusType`, this service's Smithy model).
pub const REPLICA_STATUS_IN_PROGRESS: &str = "InProgress";

/// `ReplicationStatusType$Status` once the replica exists.
pub const REPLICA_STATUS_IN_SYNC: &str = "InSync";

/// How long a fresh replica stays `InProgress` before the clock owes it the move
/// to `InSync`.
///
/// Secrets Manager documents no duration for creating a replica, so this span is
/// the emulator's OWN: long enough that a caller polling straight after
/// `ReplicateSecretToRegions` still observes `InProgress`, short enough that a
/// client waiting for `InSync` does not wait on a human timescale.
pub const REPLICA_SYNC_DELAY: Duration = Duration::from_millis(750);

/// The characters a secret name may carry besides ASCII letters and digits:
/// "The secret name can contain ASCII letters, numbers, and the following
/// characters: /_+=.@-" (`CreateSecretRequest$Name`, this service's Smithy
/// model). The model declares no pattern for `NameType`, so the documented
/// charset is enforced in the handler.
pub const SECRET_NAME_EXTRA_CHARS: &str = "/_+=.@-";

/// Whether `name` is a secret name this service accepts.
pub fn is_valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SECRET_NAME_EXTRA_CHARS.contains(c))
}

/// The region this request is addressed to, taken from its SigV4 credential
/// scope so an ARN reports the region the caller actually signed for.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Render a stored instant as the wire timestamp type.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The instant a recovery window of `days` started at `from` elapses.
///
/// TOTAL for any value a request can carry. `DeleteSecret` already refuses a
/// window outside the documented 7..30 range with the modelled
/// `InvalidParameterException`, so the clamp here is defence in depth; it keeps
/// the multiplication and the panicking `SystemTime + Duration` out of reach of
/// any caller, whatever a future caller of this function passes.
pub fn after_days(from: SystemTime, days: i64) -> SystemTime {
    let days = days.clamp(MIN_RECOVERY_WINDOW_DAYS, MAX_RECOVERY_WINDOW_DAYS);
    let window = ::std::time::Duration::from_secs(
        u64::try_from(days)
            .unwrap_or(0)
            .saturating_mul(SECONDS_PER_DAY),
    );
    from.checked_add(window).unwrap_or(from)
}

/// A fresh version id. A UUID satisfies `SecretVersionIdType`'s modelled length
/// bound of 32..64 characters.
pub fn new_version_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The six random characters that end a secret's ARN.
pub fn new_arn_suffix() -> String {
    // The alphabet Secrets Manager draws the ARN suffix from: ASCII letters and
    // digits.
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    ::uuid::Uuid::new_v4().as_bytes()[..ARN_SUFFIX_LEN]
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect()
}

/// Encode a pagination cursor — the name of the first secret of the NEXT page —
/// as the opaque `NextToken` a client hands back unchanged.
pub fn encode_next_token(name: &str) -> String {
    ::hex::encode(name.as_bytes())
}

/// Read a cursor produced by [`encode_next_token`]. `None` for a token this
/// service did not mint, which the caller answers with
/// `InvalidNextTokenException`.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// A tag as stored: both halves are optional, exactly as the modelled `Tag`
/// shape has them, so a tag round-trips through state unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredTag {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl StoredTag {
    /// The stored form of a modelled `Tag`.
    pub fn from_modeled(tag: &Tag) -> Self {
        Self {
            key: tag.key.clone(),
            value: tag.value.clone(),
        }
    }
}

/// One `ExternalSecretRotationMetadata` item as stored: the same optional
/// key/value pair the modelled `ExternalSecretRotationMetadataItem` has, so the
/// metadata a partner specified round-trips through state unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredKeyValue {
    pub key: Option<String>,
    pub value: Option<String>,
}

impl StoredKeyValue {
    /// The stored form of a modelled metadata item.
    pub fn from_modeled(item: &ExternalSecretRotationMetadataItem) -> Self {
        Self {
            key: item.key.clone(),
            value: item.value.clone(),
        }
    }

    /// The metadata item for the wire.
    pub fn to_modeled(&self) -> ExternalSecretRotationMetadataItem {
        ExternalSecretRotationMetadataItem {
            key: self.key.clone(),
            value: self.value.clone(),
        }
    }
}

/// A secret's rotation schedule as stored: every member of the modelled
/// `RotationRulesType`, so the rules round-trip through state unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredRotationRules {
    pub automatically_after_days: Option<i64>,
    pub duration: Option<String>,
    pub schedule_expression: Option<String>,
}

impl StoredRotationRules {
    /// The stored form of a modelled `RotationRulesType`.
    pub fn from_modeled(rules: &RotationRulesType) -> Self {
        Self {
            automatically_after_days: rules.automatically_after_days,
            duration: rules.duration.clone(),
            schedule_expression: rules.schedule_expression.clone(),
        }
    }

    /// The rules for the wire.
    pub fn to_modeled(&self) -> RotationRulesType {
        RotationRulesType {
            automatically_after_days: self.automatically_after_days,
            duration: self.duration.clone(),
            schedule_expression: self.schedule_expression.clone(),
        }
    }
}

/// One replica of a secret: the Region it is in, the KMS key it is encrypted
/// with, and the `StatusType` its creation has reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replica {
    pub region: String,
    pub kms_key_id: String,
    /// One of [`REPLICA_STATUS_IN_PROGRESS`] or [`REPLICA_STATUS_IN_SYNC`].
    pub status: String,
    /// The instant the `InProgress` -> `InSync` move the clock owes this replica
    /// becomes due; `None` once it has been made. Stored rather than slept on,
    /// so nothing blocks a request.
    pub in_sync_due: Option<SystemTime>,
}

impl Replica {
    /// A replica whose creation has just started in `region`.
    fn creating(region: String, kms_key_id: String, at: SystemTime) -> Self {
        Self {
            region,
            kms_key_id,
            status: REPLICA_STATUS_IN_PROGRESS.to_string(),
            in_sync_due: at.checked_add(REPLICA_SYNC_DELAY),
        }
    }

    /// Materialise the move to `InSync` once the clock has made it due. Returns
    /// whether anything changed, so a caller can tell a settled replica from an
    /// untouched one.
    fn settle(&mut self, now: SystemTime) -> bool {
        if self.in_sync_due.is_none_or(|due| due > now) {
            return false;
        }
        self.status = REPLICA_STATUS_IN_SYNC.to_string();
        self.in_sync_due = None;
        true
    }
}

/// How many tags a secret already carrying `existing` would carry once `tags`
/// were attached, which is what [`TAGS_PER_SECRET`] bounds: a key already
/// attached is replaced, not added, so it does not grow the list.
///
/// Linear in both lists: the keys go into a set rather than being rescanned per
/// input tag, so counting a long request list costs no more than reading it.
pub fn tag_count_after_attaching(existing: &[StoredTag], tags: &[Tag]) -> usize {
    let mut keys: BTreeSet<Option<&String>> = existing.iter().map(|tag| tag.key.as_ref()).collect();
    keys.extend(tags.iter().map(|tag| tag.key.as_ref()));
    keys.len()
}

/// How many replicas a secret already holding `existing` would hold once
/// `regions` were replicated to, which is what [`REPLICAS_PER_SECRET`] bounds: a
/// Region already replicated to is replaced, not added.
pub fn replica_count_after_adding(existing: &[Replica], regions: &[&str]) -> usize {
    let mut seen: BTreeSet<&str> = existing
        .iter()
        .map(|replica| replica.region.as_str())
        .collect();
    seen.extend(regions.iter().copied());
    seen.len()
}

/// One version of a secret: its value, and the staging labels pointing at it.
#[derive(Debug, Clone)]
pub struct SecretVersion {
    pub version_id: String,
    pub secret_string: Option<String>,
    pub secret_binary: Option<Vec<u8>>,
    pub version_stages: Vec<String>,
    pub created_date: SystemTime,
}

impl SecretVersion {
    /// Whether this version carries `stage`.
    pub fn has_stage(&self, stage: &str) -> bool {
        self.version_stages.iter().any(|held| held == stage)
    }

    /// Whether this version already holds exactly the value in a request — the
    /// test an `@idempotencyToken` replay turns on.
    pub fn holds_same_value(
        &self,
        secret_string: Option<&String>,
        secret_binary: Option<&Vec<u8>>,
    ) -> bool {
        self.secret_string.as_ref() == secret_string && self.secret_binary.as_ref() == secret_binary
    }

    /// The version's staging labels for the wire, omitted while it carries none
    /// (`SecretVersionStagesType` is modelled with a minimum length of 1).
    pub fn stages_out(&self) -> Option<Vec<String>> {
        (!self.version_stages.is_empty()).then(|| self.version_stages.clone())
    }
}

/// A stored secret: its metadata and every version it still holds.
#[derive(Debug, Clone)]
pub struct Secret {
    pub name: String,
    pub arn: String,
    pub region: String,
    pub description: Option<String>,
    pub kms_key_id: Option<String>,
    /// The `Type` of an externally-held secret; `None` for a secret this
    /// service stores itself.
    pub secret_type: Option<String>,
    /// The service that created the secret, for a service-linked secret;
    /// `None` for one a caller created.
    pub owning_service: Option<String>,
    pub tags: Vec<StoredTag>,
    pub created_date: SystemTime,
    pub last_changed_date: SystemTime,
    pub last_accessed_date: Option<SystemTime>,
    /// The instant the secret is scheduled to be permanently deleted;
    /// `Some(..)` means it is scheduled for deletion. This is the `DeletionDate`
    /// the service reports, and for a `ForceDeleteWithoutRecovery` deletion —
    /// which skips the recovery window — it is the instant of the delete call
    /// itself.
    pub deleted_date: Option<SystemTime>,
    /// The instant this store drops the secret for good, which [`expire_due`]
    /// materialises. It is the end of the recovery window for a windowed
    /// deletion, and a short span after the call for a
    /// `ForceDeleteWithoutRecovery` one: "Secrets Manager performs the actual
    /// deletion with an asynchronous background process, so there might be a
    /// short delay before the secret is permanently deleted."
    /// (`DeleteSecretRequest$ForceDeleteWithoutRecovery`, this service's Smithy
    /// model.) Until it falls due the secret still exists, marked for deletion.
    ///
    /// [`expire_due`]: ComAmazonawsSecretsmanagerSecretsmanagerState::expire_due
    pub purge_at: Option<SystemTime>,
    pub rotation_enabled: bool,
    /// The ARN of the Lambda rotation function `RotateSecret` configured for
    /// this secret; `None` for a secret no rotation has been configured for.
    pub rotation_lambda_arn: Option<String>,
    /// The rotation schedule `RotateSecret` configured.
    pub rotation_rules: Option<StoredRotationRules>,
    /// "The metadata needed to successfully rotate a managed external secret",
    /// as the partner specified it.
    pub external_rotation_metadata: Vec<StoredKeyValue>,
    /// The role that lets Secrets Manager rotate a secret a third-party partner
    /// holds.
    pub external_rotation_role_arn: Option<String>,
    /// The instant the last rotation of this secret COMPLETED; `None` while no
    /// rotation has ever completed, which is what a started rotation that has
    /// not finished leaves it at.
    pub last_rotated_date: Option<SystemTime>,
    /// The instant the rotation schedule next makes a rotation due.
    pub next_rotation_date: Option<SystemTime>,
    /// The version id a started rotation has staked `AWSPENDING` on, while the
    /// rotation function has not created that version yet: "During rotation,
    /// Secrets Manager creates an AWSPENDING version ID before creating the new
    /// secret version" (`DescribeSecretResponse$VersionIdsToStages`, this
    /// service's Smithy model).
    pub pending_rotation_version_id: Option<String>,
    /// The replicas of this secret, in the order their Regions were first
    /// replicated to. Empty for a secret that does not participate in
    /// cross-region replication.
    pub replicas: Vec<Replica>,
    /// Versions in the order they were created.
    pub versions: Vec<SecretVersion>,
}

impl Secret {
    /// A new secret with no versions, owned by `region`.
    pub fn new(name: String, arn: String, region: String, at: SystemTime) -> Self {
        Self {
            name,
            arn,
            region,
            description: None,
            kms_key_id: None,
            secret_type: None,
            owning_service: None,
            tags: Vec::new(),
            created_date: at,
            last_changed_date: at,
            last_accessed_date: None,
            deleted_date: None,
            purge_at: None,
            rotation_enabled: false,
            rotation_lambda_arn: None,
            rotation_rules: None,
            external_rotation_metadata: Vec::new(),
            external_rotation_role_arn: None,
            last_rotated_date: None,
            next_rotation_date: None,
            pending_rotation_version_id: None,
            replicas: Vec::new(),
            versions: Vec::new(),
        }
    }

    /// Whether a `DeleteSecret` call has scheduled this secret for deletion.
    pub fn scheduled_for_deletion(&self) -> bool {
        self.deleted_date.is_some()
    }

    /// Mark this secret for deletion: `at` is the `DeletionDate` the service
    /// reports, and `purge_at` the instant the store owes its permanent removal.
    pub fn schedule_deletion(&mut self, at: SystemTime, purge_at: SystemTime) {
        self.deleted_date = Some(at);
        self.purge_at = Some(purge_at);
    }

    /// The version with this id, deprecated ones included.
    pub fn version(&self, version_id: &str) -> Option<&SecretVersion> {
        self.versions.iter().find(|v| v.version_id == version_id)
    }

    /// The one version carrying `stage`.
    pub fn version_with_stage(&self, stage: &str) -> Option<&SecretVersion> {
        self.versions.iter().find(|v| v.has_stage(stage))
    }

    /// Every version that still carries a staging label, mapped to its labels.
    /// A version with no label is deprecated and is left out, which is what
    /// makes this map the live staging state rather than a version history.
    ///
    /// A rotation that has staked `AWSPENDING` on a version its rotation
    /// function has not created yet is in the map too, under the id it staked:
    /// "During rotation, Secrets Manager creates an AWSPENDING version ID
    /// before creating the new secret version. To check if a secret version
    /// exists, call GetSecretValue"
    /// (`DescribeSecretResponse$VersionIdsToStages`, this service's Smithy
    /// model) — so the label is reported before the version it will name is
    /// readable.
    pub fn versions_to_stages(&self) -> Option<HashMap<String, Vec<String>>> {
        let mut map: HashMap<String, Vec<String>> = self
            .versions
            .iter()
            .filter(|v| !v.version_stages.is_empty())
            .map(|v| (v.version_id.clone(), v.version_stages.clone()))
            .collect();
        if let Some(reserved) = self.reserved_pending_version_id() {
            map.insert(reserved.to_string(), vec![STAGE_PENDING.to_string()]);
        }
        (!map.is_empty()).then_some(map)
    }

    /// The version id a started rotation staked `AWSPENDING` on and whose
    /// version does not exist yet; `None` once the rotation function has created
    /// it, which is when the label lives on the version itself.
    pub fn reserved_pending_version_id(&self) -> Option<&str> {
        self.pending_rotation_version_id
            .as_deref()
            .filter(|id| self.version(id).is_none())
    }

    /// The version id `AWSPENDING` names: the version carrying the label, or the
    /// id a started rotation reserved for the version its rotation function has
    /// not created yet.
    pub fn pending_version_id(&self) -> Option<&str> {
        self.version_with_stage(STAGE_PENDING)
            .map(|version| version.version_id.as_str())
            .or_else(|| self.reserved_pending_version_id())
    }

    /// Whether a rotation of this secret is still in progress.
    ///
    /// "When rotation is successful, the AWSPENDING staging label might be
    /// attached to the same version as the AWSCURRENT version, or it might not
    /// be attached to any version. If the AWSPENDING staging label is present
    /// but not attached to the same version as AWSCURRENT, then any later
    /// invocation of RotateSecret assumes that a previous rotation request is
    /// still in progress" (`RotateSecret`, this service's Smithy model).
    pub fn rotation_in_progress(&self) -> bool {
        match self.pending_version_id() {
            Some(pending) => {
                self.version_with_stage(STAGE_CURRENT)
                    .map(|current| current.version_id.as_str())
                    != Some(pending)
            }
            None => false,
        }
    }

    /// The interval the stored rules rotate the secret on, for recomputing the
    /// next due instant. Rules are validated before they are stored, so rules
    /// that no longer parse, and a `cron()` schedule, name no interval.
    pub fn rotation_interval(&self) -> Option<Duration> {
        parse_rotation_rules(&self.rotation_rules.as_ref()?.to_modeled())
            .ok()
            .flatten()
    }

    /// The stored rotation rules for the wire.
    pub fn rotation_rules_out(&self) -> Option<RotationRulesType> {
        self.rotation_rules
            .as_ref()
            .map(StoredRotationRules::to_modeled)
    }

    /// The stored external rotation metadata for the wire, omitted while the
    /// secret carries none.
    pub fn external_rotation_metadata_out(
        &self,
    ) -> Option<Vec<ExternalSecretRotationMetadataItem>> {
        (!self.external_rotation_metadata.is_empty()).then(|| {
            self.external_rotation_metadata
                .iter()
                .map(StoredKeyValue::to_modeled)
                .collect()
        })
    }

    /// The secret's tags for the wire, omitted while it carries none.
    pub fn tags_out(&self) -> Option<Vec<Tag>> {
        (!self.tags.is_empty()).then(|| {
            self.tags
                .iter()
                .map(|tag| Tag {
                    key: tag.key.clone(),
                    value: tag.value.clone(),
                })
                .collect()
        })
    }

    /// Attach `tags` to the secret, appending the keys it does not carry yet and
    /// replacing the value of each key it does.
    ///
    /// `TagResource` "appends tags to the existing list of tags", and a tag's
    /// key is its identity ("The key identifier, or name, of the tag",
    /// `Tag$Key`), so a key that is already attached is updated in place rather
    /// than attached twice. Tagging modifies the secret's metadata, which is
    /// what `LastChangedDate` reports ("The last date and time that this secret
    /// was modified in any way").
    pub fn attach_tags(&mut self, tags: &[Tag], at: SystemTime) {
        for tag in tags {
            let stored = StoredTag::from_modeled(tag);
            match self
                .tags
                .iter_mut()
                .find(|existing| existing.key == stored.key)
            {
                Some(existing) => existing.value = stored.value,
                None => self.tags.push(stored),
            }
        }
        self.last_changed_date = at;
    }

    /// Detach every tag whose key is named, leaving the rest in place.
    ///
    /// `UntagResource` "is idempotent. If a requested tag is not attached to the
    /// secret, no error is returned and the secret metadata is unchanged", so a
    /// key that matches nothing is not an error — and when nothing matches,
    /// `LastChangedDate` does not move either.
    pub fn detach_tags(&mut self, keys: &[String], at: SystemTime) {
        let before = self.tags.len();
        self.tags
            .retain(|tag| tag.key.as_ref().is_none_or(|key| !keys.contains(key)));
        if self.tags.len() != before {
            self.last_changed_date = at;
        }
    }

    /// How many tags this secret would carry once `tags` were attached, which is
    /// what [`TAGS_PER_SECRET`] bounds.
    pub fn tag_count_after(&self, tags: &[Tag]) -> usize {
        tag_count_after_attaching(&self.tags, tags)
    }

    /// The replica held for `region`, if any.
    pub fn replica(&self, region: &str) -> Option<&Replica> {
        self.replicas.iter().find(|replica| replica.region == region)
    }

    /// How many replicas this secret would hold once `regions` were replicated
    /// to, which is what [`REPLICAS_PER_SECRET`] bounds.
    pub fn replica_count_after(&self, regions: &[&str]) -> usize {
        replica_count_after_adding(&self.replicas, regions)
    }

    /// Record a replication attempt into `region`.
    ///
    /// The replica starts `InProgress`, which the clock moves to `InSync` (see
    /// [`settle_replicas`]); a Region that already holds a replica OF THIS
    /// SECRET is replicated again, which starts its creation over rather than
    /// adding a second entry for the same Region.
    ///
    /// Nothing here reports `Failed`: that status is for a replica the service
    /// could not create, and the conflict
    /// `ReplicateSecretToRegionsRequest$ForceOverwriteReplicaSecret` resolves
    /// ("whether to overwrite a secret with the same name in the destination
    /// Region") is a DIFFERENT secret of this name in the destination Region,
    /// which this store cannot hold — a name identifies at most one secret in
    /// it. A status for a failure nothing suffered would be fabricated, so the
    /// emulator reports none.
    ///
    /// [`settle_replicas`]: Secret::settle_replicas
    pub fn replicate_to(&mut self, region: &str, kms_key_id: Option<&str>, at: SystemTime) {
        let key = kms_key_id
            .filter(|key| !key.is_empty())
            .unwrap_or(DEFAULT_KMS_KEY_ALIAS)
            .to_string();
        let fresh = Replica::creating(region.to_string(), key, at);
        match self
            .replicas
            .iter_mut()
            .find(|replica| replica.region == region)
        {
            Some(existing) => *existing = fresh,
            None => self.replicas.push(fresh),
        }
    }

    /// Stop replicating to `region`, reporting whether a replica was held there.
    pub fn remove_replica(&mut self, region: &str) -> bool {
        let before = self.replicas.len();
        self.replicas.retain(|replica| replica.region != region);
        self.replicas.len() != before
    }

    /// Materialise every replica transition the clock has already made due,
    /// reporting whether any moved. This is the move from `InProgress` to
    /// `InSync` that was owed at this instant whether or not anybody called, so
    /// a reporting operation may perform it before it answers.
    pub fn settle_replicas(&mut self, now: SystemTime) -> bool {
        let mut settled = false;
        for replica in &mut self.replicas {
            settled |= replica.settle(now);
        }
        settled
    }

    /// The secret's replication view for the wire — one `ReplicationStatusType`
    /// per replica — omitted while the secret has no replicas.
    ///
    /// `StatusMessage` is "Status message such as 'Secret with this name already
    /// exists in this region'" — the reason a replica was not created, which
    /// nothing in this emulator produces (see [`replicate_to`]), so it is
    /// omitted. `LastAccessedDate` is "the date that the secret was last
    /// accessed in the Region. This field is omitted if the secret has never
    /// been retrieved in the Region": this emulator serves `GetSecretValue` from
    /// the primary, so no call has ever retrieved a replica in its own Region.
    ///
    /// [`replicate_to`]: Secret::replicate_to
    pub fn replication_status_out(&self) -> Option<Vec<ReplicationStatusType>> {
        (!self.replicas.is_empty()).then(|| self.replication_status())
    }

    /// The same view, as a list that is EMPTY rather than absent for a secret
    /// with no replicas — what `RemoveRegionsFromReplication` answers with,
    /// since its `ReplicationStatus` is "the status of replicas for this secret
    /// after you remove Regions" and "none left" is that status.
    pub fn replication_status(&self) -> Vec<ReplicationStatusType> {
        self.replicas
            .iter()
            .map(|replica| ReplicationStatusType {
                region: Some(replica.region.clone()),
                kms_key_id: Some(replica.kms_key_id.clone()),
                status: Some(replica.status.clone()),
                status_message: None,
                last_accessed_date: None,
            })
            .collect()
    }

    /// The secret's `PrimaryRegion` for the wire.
    ///
    /// `PrimaryRegion` is part of the replication view — "The Region the secret
    /// is in. If a secret is replicated to other Regions, the replicas are
    /// listed in ReplicationStatus" — and Secrets Manager "only returns fields
    /// that have a value in the response", so a secret that is not a replication
    /// primary has no primary Region to report and the member is omitted. It
    /// comes back as soon as the secret has a replica, and goes away again once
    /// the last one is removed.
    pub fn primary_region_out(&self) -> Option<String> {
        (!self.replicas.is_empty()).then(|| self.region.clone())
    }

    /// Whether another version fits under [`VERSIONS_PER_SECRET`], given the
    /// staging labels the new version would claim.
    ///
    /// Below the bound there is always room. At it, a version every one of whose
    /// labels this put takes away is left deprecated, and [`put_version`] evicts
    /// it to make room — except the version `AWSCURRENT` is taken from, which
    /// [`put_version`] relabels `AWSPREVIOUS` instead of deprecating, so it
    /// frees nothing. With no version to free — a caller naming a fresh label
    /// every time — the caller is refused with the modelled quota error rather
    /// than growing the store.
    ///
    /// [`put_version`]: Secret::put_version
    pub fn has_room_for_version(&self, claimed: &[String]) -> bool {
        if self.versions.len() < VERSIONS_PER_SECRET {
            return true;
        }
        let demotes = claimed.iter().any(|stage| stage == STAGE_CURRENT)
            && !claimed.iter().any(|stage| stage == STAGE_PREVIOUS);
        self.versions.iter().any(|existing| {
            let freed = existing
                .version_stages
                .iter()
                .all(|held| claimed.contains(held));
            freed && !(demotes && existing.has_stage(STAGE_CURRENT))
        })
    }

    /// Add `version` and move the staging labels it claims onto it.
    ///
    /// A label belongs to one version at a time, so every label the new version
    /// claims is taken off whichever version held it; and when the new version
    /// claims `AWSCURRENT`, `AWSPREVIOUS` moves to the version `AWSCURRENT` was
    /// taken from — unless the caller placed `AWSPREVIOUS` on the new version
    /// itself, which would otherwise leave the label on two versions. A version
    /// left with no label stays stored (it is still addressable by `VersionId`)
    /// but is deprecated, and is the first thing evicted once the secret holds
    /// more than [`VERSIONS_PER_SECRET`] versions.
    pub fn put_version(&mut self, version: SecretVersion, at: SystemTime) {
        let claimed = version.version_stages.clone();
        // A rotation reserves the id of the version its rotation function will
        // create and stakes `AWSPENDING` on it, so a version arriving under that
        // id — or any version claiming that label — is that label moving onto a
        // version that now exists: the reservation has done its job, and
        // dropping it is what keeps `AWSPENDING` on one version at a time.
        if self.pending_rotation_version_id.as_deref() == Some(version.version_id.as_str())
            || claimed.iter().any(|stage| stage == STAGE_PENDING)
        {
            self.pending_rotation_version_id = None;
        }
        let demoted = if claimed.iter().any(|stage| stage == STAGE_CURRENT) {
            self.version_with_stage(STAGE_CURRENT)
                .map(|current| current.version_id.clone())
        } else {
            None
        };
        for existing in &mut self.versions {
            existing.version_stages.retain(|held| !claimed.contains(held));
        }
        if let Some(demoted) = demoted.filter(|_| !claimed.iter().any(|s| s == STAGE_PREVIOUS)) {
            for existing in &mut self.versions {
                existing.version_stages.retain(|held| held != STAGE_PREVIOUS);
            }
            if let Some(existing) = self
                .versions
                .iter_mut()
                .find(|v| v.version_id == demoted)
            {
                existing.version_stages.push(STAGE_PREVIOUS.to_string());
            }
        }
        self.versions.push(version);
        self.last_changed_date = at;
        // Keep the secret bounded: evict the oldest deprecated versions, the
        // ones no staging label reaches, until the count is back under the
        // bound. A labelled version is never evicted, which is why a put that
        // frees none is refused by `has_room_for_version` before it gets here.
        while self.versions.len() > VERSIONS_PER_SECRET {
            let oldest_deprecated = self
                .versions
                .iter()
                .position(|existing| existing.version_stages.is_empty());
            match oldest_deprecated {
                Some(index) => {
                    self.versions.remove(index);
                }
                None => break,
            }
        }
    }

    /// Apply the rotation configuration a `RotateSecret` call carries.
    ///
    /// A member the call does not name keeps the value already stored: "If you
    /// include the configuration parameters, the operation sets the values for
    /// the secret and then immediately starts a rotation. If you don't include
    /// the configuration parameters, the operation starts a rotation with the
    /// values already stored in the secret" (`RotateSecret`, this service's
    /// Smithy model). Configuring rotation is what enables it, and the next due
    /// instant is recomputed from whichever schedule the secret now holds.
    pub fn configure_rotation(&mut self, config: RotationConfig, at: SystemTime) {
        if let Some(lambda_arn) = config.lambda_arn {
            self.rotation_lambda_arn = Some(lambda_arn);
        }
        if let Some(rules) = config.rules {
            self.rotation_rules = Some(rules);
        }
        if let Some(metadata) = config.metadata {
            self.external_rotation_metadata = metadata;
        }
        if let Some(role_arn) = config.role_arn {
            self.external_rotation_role_arn = Some(role_arn);
        }
        self.rotation_enabled = true;
        self.next_rotation_date = self
            .rotation_interval()
            .and_then(|interval| at.checked_add(interval));
        self.last_changed_date = at;
    }

    /// Stake `AWSPENDING` on `version_id`, the id the rotation that is starting
    /// will create its new version of the secret under.
    ///
    /// A staging label names at most one version of a secret at a time, so the
    /// label is taken off whichever version holds it — which is how a rotation
    /// that starts where the previous one left `AWSPENDING` on the `AWSCURRENT`
    /// version (the documented "rotation succeeded" shape) still leaves the
    /// label naming one version.
    pub fn start_rotation(&mut self, version_id: String, at: SystemTime) {
        for existing in &mut self.versions {
            existing.version_stages.retain(|held| held != STAGE_PENDING);
        }
        self.pending_rotation_version_id = Some(version_id);
        self.last_changed_date = at;
    }
}

/// The rotation configuration one `RotateSecret` call carries: a member it does
/// not name is `None` and leaves what the secret already stores in place.
#[derive(Debug, Clone, Default)]
pub struct RotationConfig {
    pub lambda_arn: Option<String>,
    pub rules: Option<StoredRotationRules>,
    pub metadata: Option<Vec<StoredKeyValue>>,
    pub role_arn: Option<String>,
}

/// Per-service state for com-amazonaws-secretsmanager-secretsmanager. A handler reaches it through [`service_state`].
///
/// Secrets are held in ONE store rather than one per region: the region a secret
/// is homed in is recorded on it (and reported as `PrimaryRegion` once the
/// secret has a replica), but a `SecretId` resolves across the whole store, so a
/// secret created under one signed region is visible to a caller signing for
/// another. That is a simplification — the real service's namespace is
/// per-region — and it is the same store the quota in [`SECRET_QUOTA`] bounds.
#[derive(Debug, Default)]
pub struct ComAmazonawsSecretsmanagerSecretsmanagerState {
    /// Secrets by name, in name order so `ListSecrets` pages deterministically.
    pub secrets: BTreeMap<String, Secret>,
    /// Full ARN to secret name, so a `SecretId` is accepted in either form.
    pub arn_index: HashMap<String, String>,
}

impl ComAmazonawsSecretsmanagerSecretsmanagerState {
    /// Store a secret and index its ARN.
    ///
    /// Replacing a name drops the ARN index entry of the secret it replaces, so
    /// no ARN that names nothing is left resolvable.
    pub fn insert(&mut self, secret: Secret) {
        self.arn_index
            .insert(secret.arn.clone(), secret.name.clone());
        if let Some(replaced) = self.secrets.insert(secret.name.clone(), secret) {
            let still_current = self
                .secrets
                .get(&replaced.name)
                .is_some_and(|current| current.arn == replaced.arn);
            if !still_current {
                self.arn_index.remove(&replaced.arn);
            }
        }
    }

    /// Remove a secret by name, dropping its ARN index entry with it.
    pub fn remove(&mut self, name: &str) -> Option<Secret> {
        let removed = self.secrets.remove(name)?;
        self.arn_index.remove(&removed.arn);
        Some(removed)
    }

    /// The stored name a `SecretId` names: the bare secret name, the full ARN,
    /// or the partial ARN (the ARN without the trailing hyphen and six random
    /// characters, which a caller that never saw the full ARN can still build).
    pub fn resolve(&self, secret_id: &str) -> Option<String> {
        if self.secrets.contains_key(secret_id) {
            return Some(secret_id.to_string());
        }
        if let Some(name) = self.arn_index.get(secret_id) {
            return Some(name.clone());
        }
        // The partial-ARN scan walks the name-ordered map, not the ARN hash
        // map, so the match it picks does not depend on hash iteration order.
        self.secrets
            .values()
            .find(|secret| partial_arn(&secret.arn) == secret_id)
            .map(|secret| secret.name.clone())
    }

    /// The secret a `SecretId` names.
    pub fn get(&self, secret_id: &str) -> Option<&Secret> {
        let name = self.resolve(secret_id)?;
        self.secrets.get(&name)
    }

    /// The secret a `SecretId` names, for mutation.
    pub fn get_mut(&mut self, secret_id: &str) -> Option<&mut Secret> {
        let name = self.resolve(secret_id)?;
        self.secrets.get_mut(&name)
    }

    /// Permanently remove every secret whose deletion the clock has already made
    /// due — the end of its recovery window, or the short asynchronous delay a
    /// `ForceDeleteWithoutRecovery` deletion skips the window for. This is the
    /// deletion the clock has already made due — it was owed at this instant
    /// whether or not anybody called — so a reporting operation materialises it
    /// before it answers, and never reports a secret the service would already
    /// have deleted.
    pub fn expire_due(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .secrets
            .values()
            .filter(|secret| secret.purge_at.is_some_and(|at| at <= now))
            .map(|secret| secret.name.clone())
            .collect();
        for name in due {
            self.remove(&name);
        }
    }

    /// Materialise every replica transition the clock has already made due,
    /// across every secret the store holds. Like [`expire_due`], this is a
    /// change that was owed at this instant whether or not anybody called, so an
    /// operation that reports a replication view performs it before it answers
    /// and never reports a replica as still being created once its span has
    /// elapsed.
    ///
    /// [`expire_due`]: ComAmazonawsSecretsmanagerSecretsmanagerState::expire_due
    pub fn settle_replicas_due(&mut self, now: SystemTime) {
        for secret in self.secrets.values_mut() {
            secret.settle_replicas(now);
        }
    }
}

/// An ARN with the trailing hyphen and six random characters removed; the ARN
/// unchanged when it does not end that way.
fn partial_arn(arn: &str) -> &str {
    match arn.len().checked_sub(ARN_SUFFIX_LEN + 1) {
        Some(cut) if arn.as_bytes().get(cut) == Some(&b'-') => &arn[..cut],
        _ => arn,
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecretsmanagerSecretsmanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecretsmanagerSecretsmanagerState>)
}
