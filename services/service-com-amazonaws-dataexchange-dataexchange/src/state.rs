// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-dataexchange-dataexchange service, plus the
//! helpers every handler shares: identifier minting, ARN building and the opaque
//! pagination cursor the `@paginated` list operations hand out.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;
use sha2::{Digest, Sha256};

use ::aws_smithy_schema_server::{envelope, Violation, ViolationKind};

use crate::ops::types::{
    AssetConfiguration, ConflictException, InternalServerException, JobError, OriginDetails,
    ResourceNotFoundException, ResponseDetails, ServiceLimitExceededException, ValidationException,
};

/// The service's SigV4 signing name, and so the ARN service segment.
pub const SERVICE: &str = "dataexchange";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. Matches the region the emulator's generated shared config sets.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `Origin` for a data set this account created. One of the model's `Origin`
/// values, enumerated by `crate::ops::types::ORIGIN_VALUES` (the test suite pins
/// that this constant is still a member of that set, so the two cannot drift);
/// `ENTITLED`, the other, describes a subscriber's copy of someone else's data
/// set, which only a data grant or a Marketplace subscription mints.
pub const ORIGIN_OWNED: &str = "OWNED";

/// The modelled maximum of `MaxResults` (`@range(min: 1, max: 200)` on
/// `com.amazonaws.dataexchange#MaxResults`), which is also the page bound when the
/// caller names no smaller one.
pub const MAX_PAGE_SIZE: i32 = 200;

/// The modelled minimum of `MaxResults`, from the same `@range`.
pub const MIN_PAGE_SIZE: i32 = 1;

/// The query parameter every `MaxResults` member of this service is bound to
/// (`smithy_query(query="operation", operation="ListDataSets")` reports
/// `"httpQuery": "maxResults"` on it), and so also the member path a refusal of it
/// states — the boundary walk spells a member path in lowerCamel.
pub const MAX_RESULTS_QUERY: &str = "maxResults";

/// The query parameter the `Origin` member of `ListDataSets` is bound to
/// (`smithy_query(query="operation", operation="ListDataSets")` reports
/// `"httpQuery": "origin"`).
pub const ORIGIN_QUERY: &str = "origin";

/// The model's `Origin` values IN THE ORDER THE MODEL DECLARES THEM
/// (`smithy_query(query="shape", shape="Origin")` -> `["OWNED", "ENTITLED"]`).
///
/// `crate::ops::types::ORIGIN_VALUES` is the generated catalog and is the set this
/// service CHECKS against; it is sorted, so it cannot state the set in the model's
/// own order, which is the order a refusal quotes it in. The test suite pins that
/// the two hold the same values, so this cannot drift from the model.
pub const ORIGIN_VALUES_IN_MODEL_ORDER: &[&str] = &["OWNED", "ENTITLED"];

/// One stored data set. `revisions` holds the revision records; `revision_order`
/// is their creation order, so a list page is stable across calls.
#[derive(Debug, Clone)]
pub struct DataSet {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: String,
    pub asset_type: String,
    pub origin: String,
    pub origin_details: Option<OriginDetails>,
    pub source_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub revisions: HashMap<String, Revision>,
    pub revision_order: Vec<String>,
    /// The notifications this data set has accepted, oldest first; see
    /// [`SentNotification`].
    pub notifications: Vec<SentNotification>,
}

/// One notification `SendDataSetNotification` accepted for a data set.
///
/// AWS delivers a notification to the subscribers of the data set's AWS Marketplace
/// product and models no operation that reads one back, so this record is what the
/// acceptance left behind: WHAT kind of event was notified, the provider's comment,
/// when it was accepted, and the idempotency token it was accepted under, which is
/// what lets a repeat under the same `ClientToken` be the same acceptance rather
/// than a second notification.
///
/// The notification's `Details` and `Scope` are deliberately NOT kept: they are
/// caller-sized nested collections the model bounds only in prose, nothing reads
/// them back, and the store must stay finite.
#[derive(Debug, Clone)]
pub struct SentNotification {
    /// One of the model's `NotificationType` values, enumerated by
    /// `crate::ops::types::NOTIFICATION_TYPE_VALUES`.
    pub r#type: String,
    /// The provider's free-form comment, bounded by the model's own
    /// `@length(0..4096)` on `__stringMin0Max4096`.
    pub comment: Option<String>,
    /// The caller's `ClientToken`, bounded by the model's own `@length(1..64)`.
    pub client_token: Option<String>,
    pub accepted_at: DateTime,
}

/// One stored revision of a data set.
#[derive(Debug, Clone)]
pub struct Revision {
    pub id: String,
    pub arn: String,
    pub data_set_id: String,
    pub comment: Option<String>,
    pub finalized: bool,
    pub revoked: bool,
    pub revocation_comment: Option<String>,
    pub revoked_at: Option<DateTime>,
    pub source_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

/// One stored job: what the caller asked for, in the response form every job
/// operation reports it in, plus where it is in its lifecycle.
///
/// `details` is kept as the modelled `ResponseDetails` rather than as the request
/// form, because that is what `CreateJob`, `GetJob` and `ListJobs` all answer with;
/// keeping one representation is what makes those three agree. `data_set_id` and
/// `revision_id` are lifted out of those details so a reader can filter on them
/// without re-matching on which detail member the job carries.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub arn: String,
    /// One of the model's `Type` values, enumerated by
    /// `crate::ops::types::TYPE_VALUES`.
    pub r#type: String,
    /// One of the model's `State` values, enumerated by
    /// `crate::ops::types::STATE_VALUES`.
    pub state: String,
    pub asset_configuration: Option<AssetConfiguration>,
    pub details: ResponseDetails,
    /// The errors the job collected while it ran; empty until it runs.
    pub errors: Vec<JobError>,
    /// The data set the job's details name (`ListJobs` filters on it).
    pub data_set_id: String,
    /// The revision the job's details name, where the job type names exactly one.
    pub revision_id: Option<String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

/// Per-service state for com-amazonaws-dataexchange-dataexchange. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDataexchangeDataexchangeState {
    /// Every data set this emulator holds, keyed by its `DataSetId`.
    pub data_sets: HashMap<String, DataSet>,
    /// Creation order of `data_sets`, so `ListDataSets` pages deterministically.
    pub data_set_order: Vec<String>,
    /// Every job this emulator holds, keyed by its `JobId`.
    pub jobs: HashMap<String, Job>,
    /// Creation order of `jobs`, so `ListJobs` pages deterministically.
    pub job_order: Vec<String>,
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDataexchangeDataexchangeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDataexchangeDataexchangeState>)
}

/// Monotonic sequence folded into every minted identifier, so two ids minted in
/// the same nanosecond still differ.
static MINT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// `ResourceType` for a data set, from the model's `ResourceType` enum
/// (`ASSET | DATA_GRANT | DATA_SET | EVENT_ACTION | JOB | REVISION`).
pub const RESOURCE_TYPE_DATA_SET: &str = "DATA_SET";

/// `ResourceType` for a revision, from the same `ResourceType` enum.
pub const RESOURCE_TYPE_REVISION: &str = "REVISION";

/// `ResourceType` for a job, from the same `ResourceType` enum.
pub const RESOURCE_TYPE_JOB: &str = "JOB";

/// The `State` a job is in before anything has started it: one of the model's
/// `State` values, enumerated by `crate::ops::types::STATE_VALUES`. A job does no
/// work until `StartJob` moves it on, so this is the state a created job carries.
pub const JOB_STATE_WAITING: &str = "WAITING";

/// The 500 a handler answers with when the shared engine is not the one this
/// service's store lives in. Never a fresh engine: that would drop the write.
pub fn store_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the dataexchange store is unavailable".to_string(),
    }
}

/// The modelled 404 for a well-formed `DataSetId` that names no stored data set.
pub fn data_set_not_found(data_set_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("The data set {data_set_id} couldn't be found."),
        resource_id: Some(data_set_id.to_string()),
        resource_type: Some(RESOURCE_TYPE_DATA_SET.to_string()),
    }
}

/// The modelled 404 for a well-formed `RevisionId` that names no revision of
/// `data_set_id`.
pub fn revision_not_found(revision_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("The revision {revision_id} couldn't be found."),
        resource_id: Some(revision_id.to_string()),
        resource_type: Some(RESOURCE_TYPE_REVISION.to_string()),
    }
}

/// The modelled 404 for a well-formed `JobId` that names no stored job.
pub fn job_not_found(job_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("The job {job_id} couldn't be found."),
        resource_id: Some(job_id.to_string()),
        resource_type: Some(RESOURCE_TYPE_JOB.to_string()),
    }
}

/// The modelled 409 for a request that cannot hold against the current state of
/// `revision_id` — a write into a revision the model's `Finalized` documentation
/// calls "this read-only state".
pub fn revision_conflict(revision_id: &str, message: impl Into<String>) -> ConflictException {
    ConflictException {
        message: message.into(),
        resource_id: Some(revision_id.to_string()),
        resource_type: Some(RESOURCE_TYPE_REVISION.to_string()),
    }
}

/// The modelled 400 for a `NextToken` that is not one this caller was issued.
///
/// Every token [`Cursor::encode`] hands out carries the account it was issued to, so
/// this is the refusal for a token naming another account AND for anything that is
/// not a token this service issued at all: neither carries the caller's account id.
/// The wording is the one AWS answers with, recorded in
/// `dataexchange.list-data-set-revisions-invalid-next-token-rejected`.
///
/// `ExceptionCause` is left unset: the model's only causes are
/// `InsufficientS3BucketPolicy` and `S3AccessDenied`, neither of which describes a
/// bad continuation token.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: "Invalid NextToken. Account Id does not match Caller's Account Id.".to_string(),
        exception_cause: None,
    }
}

/// The modelled 400 for a request parameter whose value is outside the set the model
/// admits for it: `The following parameter(s) are not valid: "origin" must be one of
/// [OWNED, ENTITLED].`
///
/// `admitted` is the model's own value set, in the model's own order, so the sentence
/// states the contract rather than a remembered example.
pub fn parameter_not_in_value_set(parameter: &str, admitted: &[&str]) -> ValidationException {
    invalid_request(format!(
        "The following parameter(s) are not valid: {parameter:?} must be one of [{}].",
        admitted.join(", ")
    ))
}

/// Whether the request carried `name` as a query parameter at all - whatever value
/// it gave it.
///
/// This is the one fact a deserialized input cannot report for a member the model
/// gives a `@default`: such a member is a bare (non-`Option`) field, so an absent
/// query parameter and one sent as the default value arrive identically. The raw
/// query string is therefore where presence is read from. A parameter name is
/// matched verbatim, which is what a client sends: the names this service binds are
/// all unreserved ASCII, so no client percent-encodes them.
pub fn carries_query_parameter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    name: &str,
) -> bool {
    ctx.parts().uri.query().is_some_and(|query| {
        query
            .split('&')
            .any(|pair| pair.split('=').next() == Some(name))
    })
}

/// The page bound the request asked for: `Some(n)` for a caller-supplied
/// `maxResults`, `None` when the request carried none.
///
/// `MaxResults` is optional, and the model gives it `@default: 0` while its own
/// `@range` starts at 1 - a default outside the member's own bound. An absent
/// optional parameter is therefore ABSENT, not that default: it names no page size,
/// so the service serves its own page, and the `@range` is checked only against a
/// value the request actually carried. That is why the member carries
/// `skip_validation` on every list input of this service (see `src/ops/types/`):
/// the boundary walk would otherwise refuse the materialised default and so refuse
/// every call that legally omits the parameter.
///
/// `deserialized` is the member as the framework built it, which is the caller's
/// value whenever the parameter was present.
///
/// The structural fix lives in the vendored codegen
/// (`vendor/smithy-macros/src/lib.rs`: `usable_default` decides which members are
/// generated non-`Option` and `default_member_fallback_expr` materialises the
/// declared default) - a `@default` outside its own member's `@range` is not a
/// usable default, so the member should be generated `Option<T>` and left absent.
/// `vendor/` is read-only from inside the emulator, so the per-service seam the
/// framework offers for exactly this (`skip_validation`, plus the handler check
/// here) takes its place.
pub fn requested_max_results(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    deserialized: i32,
) -> Result<Option<i32>, ValidationException> {
    if !carries_query_parameter(ctx, MAX_RESULTS_QUERY) {
        return Ok(None);
    }
    if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&deserialized) {
        return Err(max_results_out_of_range(deserialized));
    }
    Ok(Some(deserialized))
}

/// The `@range` refusal of a caller-supplied `maxResults`, worded exactly as the
/// framework's own boundary walk words a violated numeric bound - through that
/// crate's own [`envelope`] over its own [`Violation`] - so moving this one member's
/// check into the handler cannot change the sentence a client reads.
fn max_results_out_of_range(value: i32) -> ValidationException {
    let kind = if value < MIN_PAGE_SIZE {
        ViolationKind::MinValue(MIN_PAGE_SIZE.to_string())
    } else {
        ViolationKind::MaxValue(MAX_PAGE_SIZE.to_string())
    };
    let violation = Violation {
        path: MAX_RESULTS_QUERY.to_string(),
        kind,
        value: Some(value.to_string()),
    };
    // One violated bound is one violation, so the envelope always renders.
    invalid_request(envelope(&[violation]).unwrap_or_default())
}

/// The modelled 400 for a notification about a data set that exists and is owned by
/// the caller but is not part of an AWS Marketplace data product: there is nobody to
/// notify, because a notification goes to that product's subscribers.
///
/// The wording is the one AWS answers with, recorded in
/// `dataexchange.send-data-set-notification-error-semantics`.
pub fn data_set_not_configured_for_marketplace() -> ValidationException {
    invalid_request("Data set not configured for AWS Marketplace Data Product.")
}

/// The modelled 400 for a request the model's own constraints cannot express.
/// `message` names the member and the bound it broke, so the caller can fix the
/// request from it alone.
pub fn invalid_request(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        exception_cause: None,
    }
}

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteDataSet`, so each growth site carries a cap and nothing caller-sized
// reaches the store unchecked. A cap is named with the model's own `LimitName`
// value where the service names the limit; the VALUE is this emulator's resource
// guard, set well above any realistic use, because the model publishes none.
//
// The product is therefore finite: at most MAX_DATA_SETS data sets, each holding a
// Name of at most MAX_NAME_CHARS characters, a Description of at most
// MAX_DESCRIPTION_CHARS, at most MAX_TAGS tags of at most
// MAX_TAG_KEY_CHARS/MAX_TAG_VALUE_CHARS characters, and at most
// MAX_REVISIONS_PER_DATA_SET revisions, each itself holding a Comment the model
// already bounds at 16384 characters plus the same bounded tag map.
//
// Jobs are bounded the same way: at most MAX_JOBS of them, each holding at most
// MAX_JOB_DETAIL_ENTRIES entries in any one caller-sized detail list and at most
// MAX_TAGS asset-configuration tags (the model's own `@length(max: 50)` on
// `ListOfTag`) of at most MAX_TAG_KEY_CHARS/MAX_TAG_VALUE_CHARS characters.
// ---------------------------------------------------------------------------

/// Emulator resource guard, NOT an AWS-published value: how many data sets this
/// emulator stores. The limit is the one the model's `LimitName` enum calls
/// [`LIMIT_DATA_SETS_PER_ACCOUNT`]; the model carries no number for it.
pub const MAX_DATA_SETS: usize = 250;

/// The model's `LimitName` value for the data-set count limit, reported on the
/// modelled `ServiceLimitExceededException`.
pub const LIMIT_DATA_SETS_PER_ACCOUNT: &str = "Data sets per account";

/// Emulator resource guard, NOT an AWS-published value: how many revisions this
/// emulator stores per data set. See [`LIMIT_REVISIONS_PER_DATA_SET`].
pub const MAX_REVISIONS_PER_DATA_SET: usize = 250;

/// Emulator resource guard, NOT an AWS-published value: how many jobs this emulator
/// stores. A job is reclaimed by nothing — the model has no DeleteJob — so the
/// store needs its own ceiling. The model's `LimitName` enum publishes only
/// CONCURRENCY limits for jobs, and no number for any of them.
pub const MAX_JOBS: usize = 500;

/// Emulator resource guard, NOT an AWS-published value: how many entries a single
/// job's caller-sized detail list (asset sources, asset destinations, revision
/// destinations) may carry. The model's `LimitName` enum names these limits
/// ("Assets per import job from Amazon S3", "Asset per export job from Amazon S3",
/// "Amazon Redshift datashare assets per import job from Redshift") but carries no
/// value for any of them.
pub const MAX_JOB_DETAIL_ENTRIES: usize = 100;

/// The model's `LimitName` value for the per-data-set revision limit.
pub const LIMIT_REVISIONS_PER_DATA_SET: &str = "Revisions per data set";

/// From the `SchemaChangeDetails` shape's own documentation: "a notification can
/// have up to 100 of these".
pub const MAX_SCHEMA_CHANGES: usize = 100;

/// Emulator resource guard, NOT an AWS-published value: how many accepted
/// notifications this emulator keeps per data set. A notification is reclaimed only
/// with its data set, so the record needs its own ceiling; the model's `LimitName`
/// enum publishes no limit for notifications. The NEWEST are kept: the record is an
/// append-only log nothing reads back, so a bounded tail of it keeps the store
/// finite without refusing a call AWS accepts.
pub const MAX_NOTIFICATIONS_PER_DATA_SET: usize = 500;

/// From the `Description` member's own documentation: "A description for the data
/// set. This value can be up to 16,348 characters long."
pub const MAX_DESCRIPTION_CHARS: usize = 16_348;

/// Emulator resource guard, NOT an AWS quota: the longest `Name` this emulator
/// stores. The model constrains `Name` only as a string.
pub const MAX_NAME_CHARS: usize = 256;

/// AWS's tagging limit of 50 tags per resource.
pub const MAX_TAGS: usize = 50;

/// AWS's tagging limit on a tag key: up to 128 characters.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// AWS's tagging limit on a tag value: up to 256 characters.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Refuse a caller-sized collection whose entry count would grow the store past
/// `max`. `Err` carries the message naming the member and the bound it broke.
pub fn bound_entries(member: &str, count: usize, max: usize) -> Result<(), String> {
    if count > max {
        return Err(format!(
            "{member} may carry at most {max} entries, got {count}"
        ));
    }
    Ok(())
}

/// Refuse a caller-supplied string longer than `max` characters; see [`bound_entries`].
pub fn bound_chars(member: &str, value: &str, max: usize) -> Result<(), String> {
    let length = value.chars().count();
    if length > max {
        return Err(format!(
            "{member} may be at most {max} characters, got {length}"
        ));
    }
    Ok(())
}

/// Refuse a tag map the store will not hold: too many entries, or a key or value
/// past AWS's tagging limits.
pub fn bound_tags(tags: Option<&HashMap<String, String>>) -> Result<(), String> {
    let Some(tags) = tags else {
        return Ok(());
    };
    bound_entries("Tags", tags.len(), MAX_TAGS)?;
    for (key, value) in tags {
        bound_chars(&format!("Tags key {key:?}"), key, MAX_TAG_KEY_CHARS)?;
        bound_chars(&format!("Tags value for key {key:?}"), value, MAX_TAG_VALUE_CHARS)?;
    }
    Ok(())
}

/// The modelled 402 for a create that would take the store past [`MAX_DATA_SETS`].
pub fn data_set_limit_exceeded() -> ServiceLimitExceededException {
    ServiceLimitExceededException {
        limit_name: Some(LIMIT_DATA_SETS_PER_ACCOUNT.to_string()),
        limit_value: MAX_DATA_SETS as f64,
        message: format!(
            "The {LIMIT_DATA_SETS_PER_ACCOUNT} limit of {MAX_DATA_SETS} has been reached."
        ),
    }
}

/// Mint a fresh identifier for a data set or a revision.
///
/// The model's `com.amazonaws.dataexchange#Id` is `^[a-zA-Z0-9]{30,40}$`, so the
/// value must be 30-40 alphanumerics: this returns the first 16 bytes of a SHA-256
/// digest as 32 lowercase hex characters, which satisfies it.
///
/// The digest covers a process-monotonic sequence number, the wall clock and
/// `RandomState`'s OS-seeded key, so ids are unique within a run and unguessable
/// between runs. Nothing about the request feeds it: an identifier AWS generates
/// must not be derivable from the caller's input.
pub fn mint_id() -> String {
    use std::hash::{BuildHasher, RandomState};

    let sequence = MINT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0u128, |since| since.as_nanos());
    let entropy = RandomState::new().hash_one(sequence);

    let mut digest = Sha256::new();
    digest.update(sequence.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    digest.update(entropy.to_be_bytes());
    hex::encode(&digest.finalize()[..16])
}

/// The region to stamp into an ARN: the one the request's SigV4 credential scope
/// names, else [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// `arn:aws:dataexchange:<region>:<account>:data-sets/<data_set_id>`.
pub fn data_set_arn(region: &str, data_set_id: &str) -> String {
    ::engine::emulator_arn(SERVICE, region, &format!("data-sets/{data_set_id}"))
}

/// `arn:aws:dataexchange:<region>:<account>:jobs/<job_id>`.
pub fn job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(SERVICE, region, &format!("jobs/{job_id}"))
}

/// `arn:aws:dataexchange:<region>:<account>:data-sets/<data_set_id>/revisions/<revision_id>`.
pub fn revision_arn(region: &str, data_set_id: &str, revision_id: &str) -> String {
    ::engine::emulator_arn(
        SERVICE,
        region,
        &format!("data-sets/{data_set_id}/revisions/{revision_id}"),
    )
}

/// Now, as the modelled `Timestamp` members carry it.
pub fn now() -> DateTime {
    DateTime::from(SystemTime::now())
}

/// A decoded pagination cursor: where the next page starts.
///
/// Both halves are kept because either alone is wrong when the collection changed
/// between pages: `anchor` is the id of the last item already returned, so an
/// insertion or deletion earlier in the list cannot make a page repeat or skip an
/// item, and `offset` is the fallback for when that anchor itself was deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub offset: usize,
    pub anchor: String,
}

/// Version prefix of an encoded cursor, so a token from a different scheme is
/// recognised as unparsable rather than silently misread.
const CURSOR_VERSION: &str = "1";

/// The account a `NextToken` is issued to, and so the only account one can be
/// resumed by. This emulator serves exactly one account
/// ([`::engine::EMULATOR_ACCOUNT_ID`], the account every ARN it mints carries), so
/// that is the caller's account on every request.
pub fn caller_account() -> &'static str {
    ::engine::EMULATOR_ACCOUNT_ID
}

/// The longest token [`Cursor::encode`] can produce, as hex characters: two per
/// byte of `"<version>|<account>|<offset>|<anchor>"`, where the version is one
/// character, an account id is 12 digits, a `usize` offset is at most 20 digits, the
/// anchor is an `Id` of at most 40 characters, and the three separators are one
/// each. A longer token is not one this service issued, so [`Cursor::decode`]
/// refuses it before decoding anything.
const MAX_CURSOR_CHARS: usize = 2 * (1 + 1 + 12 + 1 + 20 + 1 + 40);

impl Cursor {
    /// The cursor that resumes after `offset` items, the last of which is `anchor`.
    pub fn new(offset: usize, anchor: impl Into<String>) -> Self {
        Self {
            offset,
            anchor: anchor.into(),
        }
    }

    /// Render the cursor as the opaque `NextToken` a caller passes back.
    ///
    /// Hex of the internal encoding: the token is alphanumeric and carries no
    /// readable structure, so a client cannot build one by hand and depend on a
    /// layout the service is free to change.
    ///
    /// The account the token is issued to travels inside it, because a page is a
    /// position in ONE account's collection: resuming it from another account would
    /// name a position in a collection the token was never minted against.
    pub fn encode(&self) -> String {
        hex::encode(format!(
            "{CURSOR_VERSION}|{}|{}|{}",
            caller_account(),
            self.offset,
            self.anchor
        ))
    }

    /// Parse a `NextToken` this caller was issued. `None` for anything else - the
    /// caller gets a `ValidationException`, never a silently different page.
    ///
    /// A token whose account is not the caller's is refused exactly like one this
    /// service never issued: see [`invalid_next_token`] for the wording both get.
    ///
    /// A token longer than one this encoder can produce is rejected before it is
    /// decoded, so the work a caller can ask of this parser is bounded here rather
    /// than only by the request-size limit.
    pub fn decode(token: &str) -> Option<Self> {
        if token.len() > MAX_CURSOR_CHARS {
            return None;
        }
        let bytes = hex::decode(token).ok()?;
        let text = String::from_utf8(bytes).ok()?;
        let mut fields = text.splitn(4, '|');
        let version = fields.next()?;
        let account = fields.next()?;
        let offset = fields.next()?.parse::<usize>().ok()?;
        let anchor = fields.next()?;
        if version != CURSOR_VERSION || account != caller_account() {
            return None;
        }
        Some(Self::new(offset, anchor))
    }
}

/// The half-open window `start..end` of `ordered` that the cursor `token` and the
/// caller's `maxResults` select.
///
/// `None` means the token is not one this caller was issued.
///
/// The returned window always satisfies `start <= end <= ordered.len()`, so a
/// caller can slice `ordered` with it: a cursor carries a caller-controlled
/// offset, and one naming a position past the end (its anchor was deleted, or it
/// was minted when the collection held more) must come back as an empty final page
/// rather than panicking at the slice. That clamp lives here so every operation
/// paging through this helper inherits it.
///
/// `max_results` is the page size the CALLER asked for, as
/// [`requested_max_results`] read it off the request: `None` means the request
/// named none, and the page is bounded by the largest the model admits
/// ([`MAX_PAGE_SIZE`]) instead. A value is clamped into the modelled range, which is
/// belt and braces - the caller's value was refused outside it before this is
/// reached.
pub fn page_window(
    ordered: &[String],
    token: Option<&str>,
    max_results: Option<i32>,
) -> Option<(usize, usize)> {
    let requested_start = match token {
        None => 0,
        Some(token) => {
            let cursor = Cursor::decode(token)?;
            match ordered.iter().position(|id| *id == cursor.anchor) {
                // The anchor is still there: resume immediately after it, whatever
                // moved around it.
                Some(index) => index.saturating_add(1),
                // The anchor was deleted between pages; fall back to the recorded
                // offset.
                None => cursor.offset,
            }
        }
    };
    let start = requested_start.min(ordered.len());
    let bound = max_results
        .unwrap_or(MAX_PAGE_SIZE)
        .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE) as usize;
    let end = ordered.len().min(start.saturating_add(bound));
    Some((start, end))
}

/// The `NextToken` to return after serving `ordered[start..end]`, or `None` when
/// that page exhausted the collection.
pub fn next_token(ordered: &[String], start: usize, end: usize) -> Option<String> {
    if end >= ordered.len() || end == start {
        return None;
    }
    Some(Cursor::new(end, ordered[end - 1].clone()).encode())
}
