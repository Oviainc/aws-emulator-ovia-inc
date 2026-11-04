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

use serde_json::Value;

use crate::ops::types::{
    ExternalSecretRotationMetadataItem, Filter, ReplicationStatusType, RotationRulesType, Tag,
};

/// The envelope a version's value is stored under, and the KMS calls that seal
/// and open it: the one place this service encrypts and decrypts a secret value,
/// so every read and every write of one goes through the secret's own key.
pub mod envelope;

/// Whether IAM lets a request to this service perform the action it asks for: the
/// one gate every operation of this service consults before it reads or writes
/// anything.
pub mod authorization;

/// The IAM condition keys a request and the secret it addresses populate, which the
/// gate above decides a statement's `Condition` against.
pub mod condition_keys;

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

/// Whether `stage` is one of the staging labels the service itself moves —
/// [`STAGE_CURRENT`], [`STAGE_PREVIOUS`] and [`STAGE_PENDING`], the three the
/// model documents — rather than one a caller named.
pub fn is_reserved_stage(stage: &str) -> bool {
    matches!(stage, STAGE_CURRENT | STAGE_PREVIOUS | STAGE_PENDING)
}

/// Whether a version claiming the staging labels `claimed` cancels a pending
/// one.
///
/// `AWSPENDING` "indicates the version of the secret that contains new secret
/// information that will become the next current version when rotation
/// finishes" (`DescribeSecretResponse$VersionIdsToStages`, this service's Smithy
/// model), so the label names a candidate for the CURRENT value. Minting a new
/// current version decides that value without that candidate, which leaves
/// nothing for the label to name: the candidate is cancelled, and the version it
/// sat on carries no label at all. A write that re-stages `AWSPENDING` on the
/// version it mints names its own candidate instead, so it cancels nothing.
fn cancels_pending_version(claimed: &[String]) -> bool {
    claimed.iter().any(|stage| stage == STAGE_CURRENT)
        && !claimed.iter().any(|stage| stage == STAGE_PENDING)
}

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

/// The message `InvalidRequestException` carries for a write that would create a
/// secret version without naming the version's id.
///
/// A write that stores a value creates a version, and "this parameter specifies
/// the unique identifier for the new version. If you use the Amazon Web Services
/// CLI or one of the Amazon Web Services SDKs to call this operation, then you
/// can leave this parameter empty. The CLI or SDK generates a random UUID for you
/// and includes it as the value for this parameter in the request. If you
/// generate a raw HTTP request to the Secrets Manager service endpoint, then you
/// must generate a ClientRequestToken and include it in the request."
/// (`CreateSecretRequest$ClientRequestToken`, and the same text on
/// `PutSecretValueRequest$ClientRequestToken` and
/// `UpdateSecretRequest$ClientRequestToken`, this service's Smithy model.) The
/// service sees only the request, so a token the client never generated is a
/// missing one: the version is refused rather than staked under an id the caller
/// never chose and cannot retry with. The wire message is this one constant
/// sentence, which interpolates nothing from the request or from state.
pub const MISSING_CLIENT_REQUEST_TOKEN_MESSAGE: &str =
    "You must provide a ClientRequestToken value. We recommend a UUID-type value.";

/// The message `InvalidRequestException` carries for an operation a caller
/// cannot perform on a secret that is inside its recovery window: the secret
/// still exists ("The secret is scheduled for deletion.",
/// `InvalidRequestException`, this service's Smithy model), so it is refused by
/// state rather than reported as missing.
pub const MARKED_FOR_DELETION_MESSAGE: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// The `InvalidParameterException` message `StopReplicationToReplica` refuses a
/// `SecretId` with that, in the Region the request is signed for, names a secret
/// which is NOT a replica — a replication primary, or a standalone secret that
/// has no replicas at all.
///
/// Secrets Manager words this refusal as ONE fixed sentence naming the class of
/// secret the call is not permitted on, and nothing else: no secret name, ARN or
/// Region from the request or from the store is interpolated into it, and nothing
/// is appended about where the call should instead have been made (a standalone
/// secret has no replica Region to redirect a caller to). Every secret refused
/// for not being a replica here — holding replicas or not — carries exactly this
/// text.
pub const NOT_A_REPLICA_MESSAGE: &str = "Operation not permitted on a primary/standalone secret.";

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

/// The message `LimitExceededException` carries for an attachment that would
/// take a version past the staging-label cap (see [`STAGES_PER_VERSION`]).
///
/// `LimitExceededException` is documented as "The request failed because it
/// would exceed one of the Secrets Manager quotas." (this service's Smithy
/// model); the wire message is this one constant sentence, which states the
/// quota rather than echoing the label or the version the request named.
pub const STAGE_LIMIT_MESSAGE: &str =
    "You exceeded the maximum number of staging labels allowed on a secret.";

/// The `ResourceNotFoundException` message for a `VersionId` the named secret
/// holds no version under.
///
/// The secret itself was found, so the missing thing is the version the request
/// named — and the caller cannot tell which of the ids it sent was wrong unless
/// the message names it. Every operation that resolves a `VersionId` answers
/// with THIS sentence.
pub fn version_not_found_message(version_id: &str) -> String {
    format!("Secrets Manager can't find the specified secret version: {version_id}")
}

/// The `ResourceNotFoundException` message for a staging label no version of the
/// named secret carries.
///
/// The secret was found and the label was not, so the message names the label
/// the request asked for. Every operation that resolves a value BY STAGE — the
/// single read and the batched one — answers with THIS sentence.
pub fn stage_not_found_message(stage: &str) -> String {
    format!("Secrets Manager can't find the specified secret value for staging label: {stage}")
}

/// The `ResourceExistsException` message for a `ClientRequestToken` that already
/// names a version of this secret holding a DIFFERENT value.
///
/// The token is the new version's id and an `@idempotencyToken`, so the same
/// token with the same value is the same request; the same token with another
/// value would modify an existing version, which no operation of this service
/// does. The message is built from the request's own token and the secret's ARN,
/// which is what identifies which of a batch of idempotent writes collided.
pub fn token_in_use_message(token: &str, arn: &str) -> String {
    format!(
        "You can't use ClientRequestToken {token} because that value is already in use for a \
         version of secret {arn}."
    )
}

/// The `LimitExceededException` message for a write that would leave a secret
/// holding more versions than this store keeps for one secret (see
/// [`VERSIONS_PER_SECRET`]).
pub fn version_quota_message() -> String {
    format!(
        "The request failed because it would exceed the maximum of {VERSIONS_PER_SECRET} versions \
         this emulator stores for one secret."
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

/// How many staging labels of its OWN a caller attaches to one version of a
/// secret before a further attachment is refused.
///
/// `SecretVersionStagesType` — the list of "all of the staging labels currently
/// attached to this version of the secret" — is modelled with a maximum length
/// of 20 (this service's Smithy model), so twenty is what one version carries.
/// The labels the service itself moves (`AWSCURRENT`, `AWSPREVIOUS`,
/// `AWSPENDING`) are not counted against it: they are attached by rotation and
/// by every write of a value, so counting them would refuse a caller's
/// twentieth label on a version that is simply current.
pub const STAGES_PER_VERSION: usize = 20;

/// How many secrets one batched read resolves, and the page size it falls back
/// to when the caller names none.
///
/// `BatchGetSecretValue` "retrieves the contents of the encrypted fields
/// SecretString or SecretBinary for up to 20 secrets", `SecretIdListType` is
/// modelled with a maximum length of 20 and `MaxResultsBatchType` with a range
/// of 1..20 (this service's Smithy model), so twenty secrets is one batch
/// whichever way the caller selects them.
pub const SECRETS_PER_BATCH: usize = 20;

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

/// The key id Secrets Manager reports for a secret version encrypted under the
/// account's default Secrets Manager key.
///
/// "If you don't specify this value, then Secrets Manager uses the key
/// `aws/secretsmanager`." (`CreateSecretRequest$KmsKeyId`, this service's
/// Smithy model.) Where the service reports the keys a version is encrypted
/// under (`SecretVersionsListEntry$KmsKeyIds`) it does not name that key by id
/// or by alias: it names it with this one token, so a caller can tell a version
/// on the account default key from one on a key of the secret's own.
pub const DEFAULT_ENCRYPTION_KEY_ID: &str = "DefaultEncryptionKey";

/// Whether `kms_key_id` names the account's default Secrets Manager key —
/// [`DEFAULT_KMS_KEY_ALIAS`] — rather than a key of the secret's own.
///
/// The alias prefix is optional in a request ("An alias is always prefixed by
/// `alias/`", `CreateSecretRequest$KmsKeyId`), so both forms of that one alias
/// name the same key, and a secret that names it is on the account default key
/// exactly as one that names no key at all is.
pub fn names_default_kms_key(kms_key_id: &str) -> bool {
    let unprefixed = |key: &str| key.strip_prefix("alias/").unwrap_or(key).to_string();
    unprefixed(kms_key_id) == unprefixed(DEFAULT_KMS_KEY_ALIAS)
}

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

/// Whether a secret satisfies one `Filter`.
///
/// A filter's `Values` are alternatives (any one matching is enough) and each
/// value may be negated with a leading `!`; separate filters all have to hold.
/// Every operation that selects secrets by filter — the listing and the batched
/// read, which share one `Filter` shape — selects with THIS function, so the two
/// cannot disagree about what a filter means.
pub fn matches_filter(secret: &Secret, filter: &Filter) -> bool {
    let Some(key) = filter.key.as_deref() else {
        return true;
    };
    let values = filter.values.as_deref().unwrap_or(&[]);
    if values.is_empty() {
        return true;
    }
    values.iter().any(|value| match value.strip_prefix('!') {
        Some(negated) => !matches_filter_value(secret, key, negated),
        None => matches_filter_value(secret, key, value),
    })
}

/// Whether a secret matches one filter value.
///
/// The per-key rules come from `Filter$Key` in this service's model: `name`,
/// `tag-key`, `tag-value`, `primary-region` and `owning-service` are
/// case-sensitive prefix matches, `description` is a prefix match that is not
/// case-sensitive, and `all` "breaks the filter value string into words and then
/// searches all attributes for matches", not case-sensitive.
fn matches_filter_value(secret: &Secret, key: &str, value: &str) -> bool {
    match key {
        "name" => secret.name.starts_with(value),
        "description" => secret
            .description
            .as_deref()
            .is_some_and(|description| starts_with_ignoring_case(description, value)),
        "tag-key" => secret
            .tags
            .iter()
            .filter_map(|tag| tag.key.as_deref())
            .any(|tag_key| tag_key.starts_with(value)),
        "tag-value" => secret
            .tags
            .iter()
            .filter_map(|tag| tag.value.as_deref())
            .any(|tag_value| tag_value.starts_with(value)),
        "primary-region" => secret.region.starts_with(value),
        "owning-service" => secret
            .owning_service
            .as_deref()
            .is_some_and(|owner| owner.starts_with(value)),
        "all" => {
            let attributes = searchable_attributes(secret);
            value.split_whitespace().all(|word| {
                attributes
                    .iter()
                    .any(|attribute| starts_with_ignoring_case(attribute, word))
            })
        }
        // A key the model does not name matches nothing.
        _ => false,
    }
}

/// The attributes the `all` filter key searches.
fn searchable_attributes(secret: &Secret) -> Vec<&str> {
    let mut attributes = vec![secret.name.as_str(), secret.region.as_str()];
    attributes.extend(secret.description.as_deref());
    attributes.extend(secret.owning_service.as_deref());
    for tag in &secret.tags {
        attributes.extend(tag.key.as_deref());
        attributes.extend(tag.value.as_deref());
    }
    attributes
}

/// A prefix match that ignores ASCII case.
///
/// Compares BYTES rather than slicing the haystack at `prefix.len()`: both
/// operands are request-derived, so a byte-indexed slice would panic whenever
/// that offset fell inside a multi-byte character of the haystack. Case folding
/// is ASCII-only, as `eq_ignore_ascii_case` makes it: a non-ASCII byte has to
/// match exactly, which is the most the model's "not case-sensitive" pins down.
fn starts_with_ignoring_case(haystack: &str, prefix: &str) -> bool {
    let (haystack, prefix) = (haystack.as_bytes(), prefix.as_bytes());
    haystack.len() >= prefix.len() && haystack[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// The wildcard a policy statement names when it grants access to every
/// principal.
pub const POLICY_WILDCARD: &str = "*";

/// What `ValidateResourcePolicy` refuses a document that fails the syntax check
/// with: that operation "[c]hecks for correct syntax in a policy" (this
/// service's Smithy model) and reports the failure as this one constant
/// sentence, naming no part of the document.
pub const VALIDATE_SYNTAX_ERROR_MESSAGE: &str = "This resource policy contains a syntax error.";

/// What a resource policy document that is not a policy document is refused
/// with: `MalformedPolicyDocumentException` is "The resource policy has syntax
/// errors." (this service's Smithy model).
///
/// One fixed sentence, naming no part of the document and no secret, and the
/// SAME sentence wherever the service speaks about that condition: the
/// `MalformedPolicyDocumentException` an attach is refused with, and the message
/// the syntax check reports on the validation channel
/// ([`VALIDATE_SYNTAX_ERROR_MESSAGE`]) — so a caller using
/// `ValidateResourcePolicy` as a pre-flight guard reads the very text the
/// refusal will carry.
pub const MALFORMED_POLICY_MESSAGE: &str = VALIDATE_SYNTAX_ERROR_MESSAGE;

/// What this service says about a resource policy that grants public access to a
/// secret: `PublicPolicyException` is "The BlockPublicPolicy parameter is set to
/// true, and the resource policy did not prevent broad access to the secret."
/// (this service's Smithy model).
///
/// One fixed sentence, naming no part of the document, no secret and no
/// principal, and the SAME sentence wherever the service speaks about that
/// condition: the `PublicPolicyException` a `BlockPublicPolicy` request is
/// refused with, and the message the broad-access check reports on the
/// validation channel ([`BROAD_ACCESS_CHECK_MESSAGE`]).
pub const PUBLIC_POLICY_MESSAGE: &str =
    "You can not attach a resource policy that grants Public access to your secret.";

/// The policy elements a statement can name a principal under. A principal is
/// either the bare wildcard string or a map from principal type to one or more
/// identifiers, so the values of every type are what a wildcard can appear in.
const PRINCIPAL_ELEMENTS: &[&str] = &["Principal", "NotPrincipal"];

/// Parse a resource policy document, or answer the
/// `MalformedPolicyDocumentException` message for one that is not a policy
/// document.
///
/// The grammar enforced is the IAM policy-document one: the document is a JSON
/// object, its `Statement` element is one statement or a non-empty list of
/// them, and every statement is an object carrying an `Effect` of `Allow` or
/// `Deny`. Everything else a statement may carry is passed through untouched,
/// so a document this emulator does not interpret still round-trips byte for
/// byte.
pub fn parse_resource_policy(document: &str) -> Result<Value, String> {
    let parsed: Value =
        ::serde_json::from_str(document).map_err(|_| MALFORMED_POLICY_MESSAGE.to_string())?;
    if !parsed.is_object() {
        return Err(MALFORMED_POLICY_MESSAGE.to_string());
    }
    let statements = policy_statements(&parsed).ok_or_else(|| MALFORMED_POLICY_MESSAGE.to_string())?;
    if statements.is_empty() {
        return Err(MALFORMED_POLICY_MESSAGE.to_string());
    }
    for statement in &statements {
        if statement_effect(statement).is_none() {
            return Err(MALFORMED_POLICY_MESSAGE.to_string());
        }
    }
    Ok(parsed)
}

/// The statements of a policy document: the single statement a `Statement`
/// object is, or the list one holds. `None` for a document whose `Statement` is
/// missing or is neither.
fn policy_statements(policy: &Value) -> Option<Vec<&Value>> {
    match policy.get("Statement")? {
        Value::Object(_) => Some(vec![&policy["Statement"]]),
        Value::Array(items) => items
            .iter()
            .all(Value::is_object)
            .then(|| items.iter().collect()),
        _ => None,
    }
}

/// A statement's effect, lower-cased, or `None` when it names neither of the
/// two the grammar allows.
fn statement_effect(statement: &Value) -> Option<String> {
    let effect = statement.get("Effect")?.as_str()?.to_ascii_lowercase();
    matches!(effect.as_str(), "allow" | "deny").then_some(effect)
}

/// Whether a policy document grants broad access to the secret, which is what
/// a request that blocks public policies refuses: "Specifies whether to block
/// resource-based policies that allow broad access to the secret, for example
/// those that use a wildcard for the principal"
/// (`PutResourcePolicyRequest$BlockPublicPolicy`, this service's Smithy model).
///
/// Broad access is an ALLOW statement whose principal is the wildcard — the
/// bare `"*"`, or `"*"` among the identifiers of any principal type it maps. A
/// policy whose every allow statement names a concrete principal, and one that
/// only denies, grant no broad access however many statements they carry.
pub fn policy_grants_public_access(policy: &Value) -> bool {
    policy_statements(policy)
        .unwrap_or_default()
        .into_iter()
        .any(|statement| {
            statement_effect(statement).as_deref() == Some("allow")
                && names_wildcard_principal(statement)
        })
}

/// Whether a statement names the wildcard principal.
fn names_wildcard_principal(statement: &Value) -> bool {
    PRINCIPAL_ELEMENTS
        .iter()
        .filter_map(|element| statement.get(element))
        .any(is_wildcard_principal)
}

/// The name reported for the check that a policy does not open the secret to
/// every principal: "ensure your resource policy does not allow broad access to
/// your secret, for example policies that use a wildcard for the principal"
/// (`ValidateResourcePolicy`, this service's Smithy model).
/// The service reports this check under the name of the request flag that
/// blocks such a policy outright, `BlockPublicPolicy`.
pub const BROAD_ACCESS_CHECK_NAME: &str = "BlockPublicPolicyCheck";

/// What that check reports: the one sentence this service says about a policy
/// granting public access to a secret, the same on the reporting channel as on
/// the refusing one ([`PUBLIC_POLICY_MESSAGE`]), so a caller using this
/// operation as a pre-flight guard reads the very text the refusal will carry.
pub const BROAD_ACCESS_CHECK_MESSAGE: &str = PUBLIC_POLICY_MESSAGE;

/// The name reported for the check that a policy leaves somebody able to reach
/// the secret: "Verifies the policy does not lock out a caller"
/// (`ValidateResourcePolicy`, this service's Smithy model).
pub const LOCKS_OUT_CALLER_CHECK_NAME: &str = "LockOutCheck";

/// What that check reports.
pub const LOCKS_OUT_CALLER_CHECK_MESSAGE: &str =
    "The resource policy locks out callers: a statement denies every principal access to the \
     secret.";

/// Every check a policy document fails, as `(check name, message)` pairs in the
/// order the operation documents its checks.
///
/// The document has already satisfied the syntax check by being parsed by
/// [`parse_resource_policy`]; what is left is the broad-access check and the
/// lock-out check, each a property of the statements the document carries.
pub fn resource_policy_check_failures(policy: &Value) -> Vec<(&'static str, &'static str)> {
    let mut failures = Vec::new();
    if policy_grants_public_access(policy) {
        failures.push((BROAD_ACCESS_CHECK_NAME, BROAD_ACCESS_CHECK_MESSAGE));
    }
    if policy_locks_out_callers(policy) {
        failures.push((LOCKS_OUT_CALLER_CHECK_NAME, LOCKS_OUT_CALLER_CHECK_MESSAGE));
    }
    failures
}

/// What a document naming a principal that does not exist is refused with: like
/// the syntax refusal, one constant sentence that names no part of the document.
pub const VALIDATE_UNSUPPORTED_PRINCIPAL_MESSAGE: &str =
    "This resource policy contains an unsupported principal.";

/// Whether a resource policy document names a principal this service cannot
/// resolve to an identity that exists.
///
/// A RESOURCE-based policy statement names the identities it applies to, and
/// each of them has to be one that EXISTS: a document whose statement names
/// none, or names an identity that cannot exist, is refused rather than reported
/// on. This emulator serves exactly one account
/// (`engine::emulator_account_id`), so an identity belonging to any other
/// account cannot exist here, and neither can an identifier that is neither an
/// account id nor an ARN. The wildcard names every principal instead of one
/// identity, and a principal under any other type (a service, a federated
/// provider, a canonical user) names no account, so neither is resolved here.
pub fn names_unsupported_policy_principal(policy: &Value) -> bool {
    policy_statements(policy)
        .unwrap_or_default()
        .into_iter()
        .any(|statement| {
            let elements: Vec<&Value> = PRINCIPAL_ELEMENTS
                .iter()
                .filter_map(|element| statement.get(element))
                .collect();
            elements.is_empty()
                || elements
                    .into_iter()
                    .flat_map(account_principals)
                    .any(|principal| !names_this_account(&principal))
        })
}

/// Every identifier of one `Principal` element that names an identity by
/// account: the bare form and the identifiers under the `AWS` type.
fn account_principals(principal: &Value) -> Vec<String> {
    match principal {
        Value::String(value) => vec![value.clone()],
        Value::Array(items) => items.iter().flat_map(account_principals).collect(),
        Value::Object(types) => types
            .get(AWS_PRINCIPAL_TYPE)
            .map(account_principals)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The principal type whose identifiers name an identity inside an account, as
/// an account id or as an ARN.
const AWS_PRINCIPAL_TYPE: &str = "AWS";

/// Whether one `AWS` principal identifier names an identity of the account this
/// service serves. The wildcard names every principal rather than one identity,
/// so it is resolvable whatever the account.
fn names_this_account(principal: &str) -> bool {
    if principal == POLICY_WILDCARD {
        return true;
    }
    principal_account(principal).is_some_and(|account| account == ::engine::emulator_account_id())
}

/// The account an `AWS` principal identifier belongs to: the identifier itself
/// when it is an account id, the account field when it is an ARN, and `None` for
/// an identifier that is neither.
fn principal_account(principal: &str) -> Option<&str> {
    if is_account_id(principal) {
        return Some(principal);
    }
    let account = principal.strip_prefix("arn:")?.split(':').nth(3)?;
    is_account_id(account).then_some(account)
}

/// The number of digits of an Amazon Web Services account id.
const ACCOUNT_ID_LEN: usize = 12;

/// Whether a value is an account id: exactly twelve digits.
fn is_account_id(value: &str) -> bool {
    value.len() == ACCOUNT_ID_LEN && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether a policy document locks every caller out of the secret: a DENY
/// statement whose `Principal` is the wildcard, carrying no `Condition` that
/// narrows which requests it denies, so the deny reaches every caller including
/// the secret's own account.
///
/// A deny that names concrete principals, or one whose reach a condition
/// narrows, leaves callers outside it able to reach the secret and locks nobody
/// out.
pub fn policy_locks_out_callers(policy: &Value) -> bool {
    policy_statements(policy)
        .unwrap_or_default()
        .into_iter()
        .any(|statement| {
            statement_effect(statement).as_deref() == Some("deny")
                && statement
                    .get("Principal")
                    .is_some_and(is_wildcard_principal)
                && statement.get("Condition").is_none()
        })
}

/// Whether one `Principal` element is the wildcard: the bare string, or a map
/// from principal type to identifiers one of which is the wildcard.
fn is_wildcard_principal(principal: &Value) -> bool {
    match principal {
        Value::String(value) => value == POLICY_WILDCARD,
        Value::Array(items) => items.iter().any(is_wildcard_principal),
        Value::Object(types) => types.values().any(is_wildcard_principal),
        _ => false,
    }
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

/// `at` truncated to midnight UTC, the resolution the service reports an access
/// date at: "Note that the resolution of this field is at the date level and
/// does not include the time." (`SecretVersionsListEntry$LastAccessedDate`,
/// this service's Smithy model.)
pub fn start_of_day(at: SystemTime) -> SystemTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    UNIX_EPOCH + Duration::from_secs(since_epoch - since_epoch % SECONDS_PER_DAY)
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
    /// The version's value, under the key the secret names: sealed under a data
    /// key a customer managed key wrapped, or held as sent for a secret on the
    /// service-managed key. See [`envelope`].
    pub value: envelope::StoredValue,
    pub version_stages: Vec<String>,
    pub created_date: SystemTime,
    /// The date this version's value was last retrieved, at the resolution the
    /// service reports it (see [`Secret::record_access`]); `None` while it has
    /// never been retrieved.
    pub last_accessed_date: Option<SystemTime>,
}

impl SecretVersion {
    /// Whether this version carries `stage`.
    pub fn has_stage(&self, stage: &str) -> bool {
        self.version_stages.iter().any(|held| held == stage)
    }

    /// Whether this version already holds exactly the value in a request — the
    /// test an `@idempotencyToken` replay turns on.
    pub fn holds_same_value(&self, value: &envelope::SecretValue) -> bool {
        self.value.holds(value)
    }

    /// The version's staging labels for the wire, omitted while it carries none
    /// (`SecretVersionStagesType` is modelled with a minimum length of 1).
    pub fn stages_out(&self) -> Option<Vec<String>> {
        (!self.version_stages.is_empty()).then(|| self.stages_in_report_order())
    }

    /// The version's staging labels in the one order the service reports them:
    /// the labels it moves itself first, in the order its own lifecycle names
    /// them ([`STAGE_CURRENT`], then [`STAGE_PENDING`], then
    /// [`STAGE_PREVIOUS`]), and then the labels a caller attached, in the order
    /// they were attached.
    ///
    /// A version can carry several labels at once — "the AWSPENDING staging
    /// label might be attached to the same version as the AWSCURRENT version"
    /// (`RotateSecret`, this service's Smithy model) — and what it reports is
    /// the same list however the labels got there, rather than the order in
    /// which they happened to be moved onto it.
    pub fn stages_in_report_order(&self) -> Vec<String> {
        let mut ordered: Vec<String> = [STAGE_CURRENT, STAGE_PENDING, STAGE_PREVIOUS]
            .into_iter()
            .filter(|stage| self.has_stage(stage))
            .map(str::to_string)
            .collect();
        ordered.extend(
            self.version_stages
                .iter()
                .filter(|stage| !is_reserved_stage(stage))
                .cloned(),
        );
        ordered
    }

    /// How many of the staging labels on this version a CALLER named: the ones
    /// the service moves itself are left out, as [`is_reserved_stage`] decides.
    /// This is the count [`STAGES_PER_VERSION`] bounds.
    pub fn custom_stage_count(&self) -> usize {
        self.version_stages
            .iter()
            .filter(|stage| !is_reserved_stage(stage))
            .count()
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
    /// The resource-based permission policy document attached to this secret,
    /// exactly as the caller supplied it; `None` for a secret no policy is
    /// attached to. `GetResourcePolicy` "retrieves the JSON text of the
    /// resource-based policy document attached to the secret" (this service's
    /// Smithy model), so the document is stored verbatim rather than
    /// re-serialized.
    pub resource_policy: Option<String>,
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
            resource_policy: None,
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

    /// Whether a scheduled deletion of this secret can still be cancelled.
    ///
    /// `DeletionDate` is "the date and time after which this secret Secrets
    /// Manager can permanently delete this secret, and it can no longer be
    /// restored" (`DeleteSecretResponse$DeletionDate`, this service's Smithy
    /// model), so a deletion is cancellable exactly while that instant is still
    /// ahead. A deletion that skipped the recovery window stamps that instant at
    /// the call itself, so there is no span in which it can be cancelled.
    pub fn deletion_cancellable(&self, now: SystemTime) -> bool {
        self.deleted_date.is_some_and(|at| at > now)
    }

    /// Cancel a scheduled deletion: drop the `DeletionDate` stamp and with it
    /// the permanent removal the store owed, leaving the secret's versions,
    /// staging labels and every other attribute as they were.
    pub fn cancel_deletion(&mut self, at: SystemTime) {
        self.deleted_date = None;
        self.purge_at = None;
        self.last_changed_date = at;
    }

    /// The version with this id, deprecated ones included.
    pub fn version(&self, version_id: &str) -> Option<&SecretVersion> {
        self.versions.iter().find(|v| v.version_id == version_id)
    }

    /// The keys every version of this secret is encrypted under: the key the
    /// secret names, or [`DEFAULT_ENCRYPTION_KEY_ID`] for a secret left on the
    /// account's default Secrets Manager key, which is the key one that names
    /// none is encrypted with (`CreateSecretRequest$KmsKeyId`, this service's
    /// Smithy model). This is `SecretVersionsListEntry$KmsKeyIds`, "the KMS
    /// keys used to encrypt the secret version", and it is never empty: every
    /// version is encrypted under some key.
    pub fn encryption_key_ids(&self) -> Vec<String> {
        match self
            .kms_key_id
            .as_deref()
            .filter(|key| !names_default_kms_key(key))
        {
            Some(key) => vec![key.to_string()],
            None => vec![DEFAULT_ENCRYPTION_KEY_ID.to_string()],
        }
    }

    /// Record that the value of the version `version_id` was retrieved at `at`.
    ///
    /// `LastAccessedDate` is "the date that the secret was last accessed in the
    /// Region. This field is omitted if the secret has never been retrieved in
    /// the Region." (`DescribeSecretResponse$LastAccessedDate`, this service's
    /// Smithy model), so it is retrieving a value that sets it — on the version
    /// whose value was read and on the secret that holds it. It is stored at
    /// the resolution the service reports it at (see [`start_of_day`]), so
    /// repeated retrievals on one day leave it where it is.
    ///
    /// Nothing else about the secret moves: a retrieval changes no value, no
    /// staging label and no attribute, so `LastChangedDate` stays put.
    pub fn record_access(&mut self, version_id: &str, at: SystemTime) {
        let date = start_of_day(at);
        if let Some(version) = self
            .versions
            .iter_mut()
            .find(|version| version.version_id == version_id)
        {
            version.last_accessed_date = Some(date);
        }
        self.last_accessed_date = Some(date);
    }

    /// The one version carrying `stage`.
    pub fn version_with_stage(&self, stage: &str) -> Option<&SecretVersion> {
        self.versions.iter().find(|v| v.has_stage(stage))
    }

    /// The stored value of every version that carries one of the staging labels
    /// the service itself moves, by version id.
    ///
    /// These are the versions a change of the secret's key moves to that key:
    /// `KmsKeyId` is "the KMS key that Secrets Manager uses to encrypt new
    /// secret versions as well as any existing versions with the staging labels
    /// AWSCURRENT, AWSPENDING, or AWSPREVIOUS" (`UpdateSecretRequest$KmsKeyId`,
    /// this service's Smithy model) — a deprecated version carries none of them
    /// and keeps the key it was sealed under.
    pub fn staged_version_values(&self) -> Vec<(String, envelope::StoredValue)> {
        self.versions
            .iter()
            .filter(|version| {
                version
                    .version_stages
                    .iter()
                    .any(|held| is_reserved_stage(held))
            })
            .map(|version| (version.version_id.clone(), version.value.clone()))
            .collect()
    }

    /// Replace the stored value of the version `version_id` with `value`, the
    /// same plaintext sealed under the key the secret is now on.
    ///
    /// Only a re-encryption reaches here: a version's VALUE is never otherwise
    /// modified ("You can't modify an existing version, you can only create a
    /// new version", `UpdateSecret`, this service's Smithy model), so what this
    /// changes is the key the bytes are held under and nothing a caller reads.
    /// A version that has since gone is left alone.
    pub fn reseal_version(&mut self, version_id: &str, value: envelope::StoredValue) {
        if let Some(version) = self
            .versions
            .iter_mut()
            .find(|version| version.version_id == version_id)
        {
            version.value = value;
        }
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
            .map(|v| (v.version_id.clone(), v.stages_in_report_order()))
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

    /// Attach `policy` to the secret, replacing whatever policy it carried.
    ///
    /// `PutResourcePolicy` "attaches a resource-based permission policy to a
    /// secret" and one secret carries at most one such policy, so a second call
    /// replaces the document rather than adding to it. Attaching one modifies
    /// the secret, which is what `LastChangedDate` reports ("The last date and
    /// time that this secret was modified in any way", this service's Smithy
    /// model).
    pub fn attach_resource_policy(&mut self, policy: String, at: SystemTime) {
        self.resource_policy = Some(policy);
        self.last_changed_date = at;
    }

    /// Detach the resource policy, reporting whether the secret carried one.
    ///
    /// `DeleteResourcePolicy` "deletes the resource-based permission policy
    /// attached to the secret" (this service's Smithy model) and models no
    /// error for a secret that carries none, so detaching nothing succeeds —
    /// and, having changed nothing, leaves `LastChangedDate` where it was.
    pub fn detach_resource_policy(&mut self, at: SystemTime) -> bool {
        let had = self.resource_policy.take().is_some();
        if had {
            self.last_changed_date = at;
        }
        had
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
    /// A put that cancels a pending version ([`cancels_pending_version`]) takes
    /// `AWSPENDING` away too, so it frees a version that carried only that
    /// label: the two have to agree about what the put takes away, or a write
    /// that does make room is refused by the quota.
    ///
    /// [`put_version`]: Secret::put_version
    pub fn has_room_for_version(&self, claimed: &[String]) -> bool {
        if self.versions.len() < VERSIONS_PER_SECRET {
            return true;
        }
        let demotes = claimed.iter().any(|stage| stage == STAGE_CURRENT)
            && !claimed.iter().any(|stage| stage == STAGE_PREVIOUS);
        let cancels_pending = cancels_pending_version(claimed);
        self.versions.iter().any(|existing| {
            let freed = existing.version_stages.iter().all(|held| {
                claimed.contains(held) || (cancels_pending && held == STAGE_PENDING)
            });
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
    ///
    /// A new version that takes `AWSCURRENT` without re-staging `AWSPENDING`
    /// also takes `AWSPENDING` off whichever version held it, and releases a
    /// rotation's reservation of it: see [`cancels_pending_version`].
    pub fn put_version(&mut self, version: SecretVersion, at: SystemTime) {
        let claimed = version.version_stages.clone();
        // Minting the current value from somewhere other than the pending
        // candidate cancels that candidate, so the version it sat on is left
        // carrying no staging label — its value stays addressable by
        // `VersionId` like any deprecated version's.
        if cancels_pending_version(&claimed) {
            for existing in &mut self.versions {
                existing.version_stages.retain(|held| held != STAGE_PENDING);
            }
            self.pending_rotation_version_id = None;
        }
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

    /// The version id `stage` currently names: the version carrying the label,
    /// and for `AWSPENDING` the id a started rotation reserved for the version
    /// its rotation function has not created yet — the reservation IS the label's
    /// current holder until that version arrives.
    pub fn stage_holder(&self, stage: &str) -> Option<&str> {
        if stage == STAGE_PENDING {
            return self.pending_version_id();
        }
        self.version_with_stage(stage)
            .map(|version| version.version_id.as_str())
    }

    /// Attach `stage` to the version `move_to` already holds.
    ///
    /// "Each staging label can be attached to only one version at a time. To add
    /// a staging label to a version when it is already attached to another
    /// version, Secrets Manager first removes it from the other version first
    /// and then attaches it to this one", and "Whenever you move AWSCURRENT,
    /// Secrets Manager automatically moves the label AWSPREVIOUS to the version
    /// that AWSCURRENT was removed from" (`UpdateSecretVersionStage`, this
    /// service's Smithy model) — so the label ends up on exactly one version,
    /// and a move of the current label leaves the previous one behind it.
    ///
    /// A move of `AWSPENDING` onto a version that exists also drops a rotation's
    /// reservation of that label, for the reason [`put_version`] drops it: the
    /// reservation stands only while the label names a version the store does
    /// not hold.
    ///
    /// [`put_version`]: Secret::put_version
    pub fn attach_stage(&mut self, stage: &str, move_to: &str, at: SystemTime) {
        let demoted = if stage == STAGE_CURRENT {
            self.version_with_stage(STAGE_CURRENT)
                .map(|current| current.version_id.clone())
                .filter(|current| current != move_to)
        } else {
            None
        };
        for existing in &mut self.versions {
            existing.version_stages.retain(|held| held != stage);
        }
        if let Some(demoted) = demoted {
            for existing in &mut self.versions {
                existing.version_stages.retain(|held| held != STAGE_PREVIOUS);
            }
            if let Some(existing) = self
                .versions
                .iter_mut()
                .find(|version| version.version_id == demoted)
            {
                existing.version_stages.push(STAGE_PREVIOUS.to_string());
            }
        }
        if let Some(existing) = self
            .versions
            .iter_mut()
            .find(|version| version.version_id == move_to)
        {
            existing.version_stages.push(stage.to_string());
        }
        if stage == STAGE_PENDING {
            self.pending_rotation_version_id = None;
        }
        self.last_changed_date = at;
    }

    /// Take `stage` off the version `remove_from`.
    ///
    /// "If this action results in the last label being removed from a version,
    /// then the version is considered to be 'deprecated' and can be deleted by
    /// Secrets Manager" (`UpdateSecretVersionStage`, this service's Smithy
    /// model), so the version stays stored and simply carries no label.
    ///
    /// Removing `AWSPENDING` from the id a cancelled rotation reserved releases
    /// that reservation, which is the clean-up the model sends a caller to this
    /// operation for.
    pub fn detach_stage(&mut self, stage: &str, remove_from: &str, at: SystemTime) {
        if let Some(existing) = self
            .versions
            .iter_mut()
            .find(|version| version.version_id == remove_from)
        {
            existing.version_stages.retain(|held| held != stage);
        }
        if stage == STAGE_PENDING
            && self.pending_rotation_version_id.as_deref() == Some(remove_from)
        {
            self.pending_rotation_version_id = None;
        }
        self.last_changed_date = at;
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

    /// Turn automatic rotation off and cancel a rotation that is still in
    /// progress, reporting the version id that rotation staked `AWSPENDING` on
    /// (`None` when no rotation was in progress).
    ///
    /// "Turns off automatic rotation, and if a rotation is currently in
    /// progress, cancels the rotation" (`CancelRotateSecret`, this service's
    /// Smithy model), so the schedule stops making rotations due. The rotation
    /// configuration itself is KEPT: "To turn on automatic rotation again, call
    /// RotateSecret", which rotates "with the values already stored in the
    /// secret" when the call names none.
    ///
    /// The staging labels the cancelled rotation left are kept too: "If you
    /// cancel a rotation in progress, it can leave the VersionStage labels in an
    /// unexpected state. You might need to remove the staging label AWSPENDING
    /// from the partially created version. ... Failing to clean up a cancelled
    /// rotation can block you from starting future rotations." — the caller
    /// cleans them up with `UpdateSecretVersionStage`, which is also why the id
    /// is reported back here.
    pub fn cancel_rotation(&mut self, at: SystemTime) -> Option<String> {
        let cancelled = self
            .rotation_in_progress()
            .then(|| self.pending_version_id().map(str::to_string))
            .flatten();
        self.rotation_enabled = false;
        self.next_rotation_date = None;
        self.last_changed_date = at;
        cancelled
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
