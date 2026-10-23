// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports service.
//!
//! The service's one durable resource is the data export: created by `CreateExport`,
//! read back by `GetExport` / `ListExports` / `ListTagsForResource`, overwritten by
//! `UpdateExport` and removed by `DeleteExport`. Every one of those handlers computes
//! its answer from [`StoredExport`] values held here, so a create is observable by every
//! later reader and a delete is observable by none.
//!
//! The ARN/region/token/timestamp helpers below live beside the store rather than in any
//! one operation file because several operations mint or parse exactly the same values,
//! and two copies of an ARN rule is how a `GetExport` stops finding what `CreateExport`
//! stored.
//!
//! Beside the store there is also the TABLE CATALOG ([`catalog`], re-exported here): the
//! tables an account can query, their table properties and their column schemas. That
//! catalog is fixed service data — the same for every account, and nothing a caller does
//! changes it — so it is a static rather than a field of the state struct: there is
//! nothing to persist and nothing to mutate, and `GetTable` (with `ListTables`, which
//! reports the same entries) must answer the same catalog on every call. It lives beside
//! the store, not inside one operation file, for the same reason the ARN rule does: two
//! operations read it. [`query`] reads a submitted `DataQuery` against that catalog, for
//! the two operations that accept an export definition.
//!
//! Two more things the store holds, each read by a layer rather than by one handler:
//!
//! * the recorded EXECUTIONS of an export ([`StoredExecution`]), which `GetExecution`
//!   reports one of and `ListExecutions` pages over. Nothing in this emulator creates
//!   one — see that type's own note.
//!
//! [`authorization`] is beside them: who a request speaks for and whether IAM lets it do
//! what it asks, for the operations whose model declares `AccessDeniedException`. It holds
//! nothing itself — the principal comes from STS's store and the policies from IAM's — so
//! it is a reader of other services' state rather than a field of this one.

use crate::ops::types::{
    ExecutionStatus, Export, ExportStatus, ResourceNotFoundException, ResourceTag,
    ValidationException, ValidationExceptionField,
};
use std::collections::BTreeMap;

pub mod authorization;
pub mod catalog;
pub mod query;

// The catalog's types, data and refusals are reached as `crate::state::…` by the two
// operations that read it, exactly as they were before the catalog grew its own module.
// The query reader is reached as `crate::state::…` by the two operations that accept an
// export definition, for the same reason the catalog is: both of them read it.
pub use query::{
    effective_table_configurations, query_tables, validate_data_query, QueryRefusal,
    TableConfigurations,
};

// The authorization decision is reached as `crate::state::…` by every operation whose
// model declares `AccessDeniedException`; see [`authorization`] for why the others are
// not judged.
pub use authorization::authorize;

pub use catalog::{
    primary_billing_view_arn, table_spec, unknown_table, ColumnGroup, ColumnSpec,
    PropertyDefault, TablePropertySpec, TableSpec, PROPERTY_ANY_VALUE,
    PROPERTY_BILLING_VIEW_ARN, PROPERTY_DEPRECATED, PROPERTY_FALSE, PROPERTY_FILTER,
    PROPERTY_INCLUDE_ALL_RECOMMENDATIONS, PROPERTY_INCLUDE_CAPACITY_RESERVATION_DATA,
    PROPERTY_INCLUDE_IAM_PRINCIPAL_DATA, PROPERTY_INCLUDE_MANUAL_DISCOUNT_COMPATIBILITY,
    PROPERTY_INCLUDE_RESOURCES, PROPERTY_INCLUDE_SPLIT_COST_ALLOCATION_DATA,
    PROPERTY_TIME_GRANULARITY, PROPERTY_TRUE, TABLE_CATALOG,
};

/// This service's SigV4 signing name, which is also its ARN namespace (see
/// `OPERATIONS.md`: "sigv4 signing name: bcm-data-exports", "ARN namespace:
/// bcm-data-exports").
pub const SIGNING_NAME: &str = "bcm-data-exports";

/// The region an ARN carries when the request is not signed, so no SigV4 credential
/// scope names one. Matches the region every generated emulator's shared config
/// defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `ResourceType` a `ResourceNotFoundException` reports for an export ARN that
/// names nothing: the Smithy shape the ARN identifies.
pub const EXPORT_RESOURCE_TYPE: &str = "Export";

/// The `ResourceType` a `ResourceNotFoundException` reports for an `ExecutionId` that
/// names no execution of the export it was asked about.
pub const EXECUTION_RESOURCE_TYPE: &str = "Execution";

/// The resource-ARN segment every export of this service is named under:
/// `export/<Export.Name>`.
pub const EXPORT_RESOURCE_PREFIX: &str = "export/";

/// The `ExportStatusCode` reported for an export that is operating normally. One of
/// `ExportStatusCode`'s two modelled values (see
/// `crate::ops::types::EXPORT_STATUS_CODE_VALUES`: `HEALTHY`, `UNHEALTHY`).
pub const EXPORT_STATUS_HEALTHY: &str = "HEALTHY";

/// The most exports this emulator retains, after which `CreateExport` answers the
/// modelled `ServiceQuotaExceededException` (HTTP 402, "You've reached the limit on the
/// number of resources you can create, or exceeded the size of an individual resource").
///
/// AWS enforces a per-account export quota through that same error; the published number
/// is not cited here, so this value is the EMULATOR's choice — set well above any
/// plausible test or demo workload. Its job is to bound the caller-driven growth of
/// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`],
/// which lives for the life of the process: without it, repeated `CreateExport` calls
/// grow the store without limit.
pub const MAX_EXPORTS: usize = 1000;

/// The `ServiceCode` a quota failure reports: this service's endpoint prefix.
pub const SERVICE_CODE: &str = SIGNING_NAME;

/// The quota a `ServiceQuotaExceededException` from `CreateExport` names.
pub const EXPORTS_PER_ACCOUNT_QUOTA_CODE: &str = "ExportsPerAccount";

/// One recorded execution of an export: the delivery AWS runs for it, as
/// `GetExecution`/`ListExecutions` report it.
///
/// NOTHING in this emulator creates one: an execution is the record of a data-plane
/// delivery (a query run against the billing data and a file written to the caller's S3
/// bucket), and this emulator runs no such delivery — so it reports no execution rather
/// than a status for work nothing performed. The store is here, and `GetExecution` reads
/// it, so an execution a delivery records is the one every reader answers from.
#[derive(Debug, Clone)]
pub struct StoredExecution {
    /// The id this execution is addressed by, unique within its export.
    pub execution_id: String,
    /// The ARN of the export that generated it.
    pub export_arn: String,
    /// The status of this execution.
    pub status: ExecutionStatus,
    /// "This export data is a snapshot from when the execution was generated"
    /// (smithy_query shape `GetExecutionResponse`, member `Export`): the definition as it
    /// stood then, which a later `UpdateExport` does not change.
    pub export: Export,
}

/// One persisted data export: the submitted definition, the server-owned status, and
/// the tags `CreateExport` associated with it.
#[derive(Debug, Clone)]
pub struct StoredExport {
    /// The server-minted ARN, which is also this export's key in
    /// [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`].
    pub arn: String,
    /// The export definition as submitted, with `ExportArn` set to [`Self::arn`].
    pub export: Export,
    /// Server-owned status and timestamps.
    pub status: ExportStatus,
    /// The tags associated with this export, keyed by tag key so a key is unique.
    pub tags: BTreeMap<String, String>,
}

/// Per-service state for com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports. A handler reaches it through [`service_state`].
///
/// There is ONE store, and no second index beside it: the ARN [`export_arn`] mints is a
/// function of the region and `Export.Name`, so the key of this map already IS the name
/// index, and `CreateExport` refuses a name in use by looking the minted ARN up here. A
/// second map keyed by the bare name would also refuse a name already used in a
/// DIFFERENT region, which the ARN says is a different export.
#[derive(Debug, Default)]
pub struct ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState {
    /// The primary store, keyed by the minted export ARN. `BTreeMap` so every list
    /// walks the exports in one stable order and a page token can name a position in it.
    pub exports: BTreeMap<String, StoredExport>,
    /// The recorded executions, keyed by `(export ARN, execution id)` — the pair
    /// `GetExecution` addresses one by. `BTreeMap` so the executions of one export are a
    /// contiguous range in one stable order, which is the order `ListExecutions` pages
    /// over.
    pub executions: BTreeMap<(String, String), StoredExecution>,
}

impl ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState {
    /// The recorded execution `execution_id` of the export `export_arn`, if there is one.
    pub fn execution(&self, export_arn: &str, execution_id: &str) -> Option<&StoredExecution> {
        self.executions
            .get(&(export_arn.to_string(), execution_id.to_string()))
    }

    /// The recorded executions OF ONE EXPORT, in execution-id order.
    ///
    /// The store is keyed by `(export ARN, execution id)`, so the executions of one export
    /// are the contiguous range that starts at that ARN's smallest possible key: the walk
    /// visits each of them exactly once, in one stable order, and visits no execution of
    /// any other export.
    pub fn executions_of<'a>(
        &'a self,
        export_arn: &'a str,
    ) -> impl Iterator<Item = &'a StoredExecution> + 'a {
        self.executions
            .range((export_arn.to_string(), String::new())..)
            .take_while(move |((arn, _), _)| arn == export_arn)
            .map(|(_, execution)| execution)
    }

    /// Record `execution`, replacing any earlier record of the same execution of the same
    /// export.
    pub fn insert_execution(&mut self, execution: StoredExecution) {
        let key = (execution.export_arn.clone(), execution.execution_id.clone());
        self.executions.insert(key, execution);
    }

    /// Forget every recorded execution of the export `export_arn`.
    ///
    /// An execution is addressed by `(ExportArn, ExecutionId)`, so an execution of an
    /// export that no longer exists can be addressed by nobody: `DeleteExport` calls this
    /// so no reader reports the history of an export the store no longer holds.
    pub fn remove_executions_of(&mut self, export_arn: &str) {
        let doomed: Vec<(String, String)> = self
            .executions
            .range((export_arn.to_string(), String::new())..)
            .take_while(|((arn, _), _)| arn == export_arn)
            .map(|(key, _)| key.clone())
            .collect();
        for key in doomed {
            self.executions.remove(&key);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState>)
}

/// The current wall-clock instant, as the modelled `Timestamp` type, at WHOLE-SECOND
/// precision.
///
/// RECORDED AWS behaviour: an `ExportStatus` timestamp comes back carrying no sub-second
/// part — the recorded instants are whole epoch seconds, and a replay of the recording
/// against an emulator emitting a fractional epoch value reports the difference in shape.
/// The instants this service records are therefore truncated here, in the ONE place that
/// mints them, rather than at each serializer, where one path could keep a fraction the
/// others drop.
///
/// The instant itself is wall-clock, so it is a per-run value: what is pinned is its
/// type, its units and its precision, not the number.
pub fn now() -> ::aws_smithy_types::DateTime {
    let seconds = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_secs() as i64);
    ::aws_smithy_types::DateTime::from_secs(seconds)
}

/// The region an ARN minted for this request carries: the region of the request's
/// SigV4 credential scope, else [`DEFAULT_REGION`].
///
/// A scope region that could not appear in the modelled `Arn` pattern's region segment
/// (`[-a-z0-9]*`) is not used, so a minted ARN always satisfies the pattern the model
/// declares for it.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region_segment(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `value` is a legal region segment of the modelled `Arn` pattern
/// `^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`.
fn is_arn_region_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The ARN of the export called `name` in `region`:
/// `arn:aws:bcm-data-exports:<region>:<account>:export/<name>`.
///
/// Built through `engine::emulator_arn`, so the account segment is the one canonical
/// emulator account id rather than a literal.
pub fn export_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("{EXPORT_RESOURCE_PREFIX}{name}"),
    )
}

/// The `Message` a `ResourceNotFoundException` of the EXPORT-DEFINITION operations
/// carries (`GetExport`, `UpdateExport`, `DeleteExport`).
///
/// Recorded AWS behavior: BCM Data Exports answers a lookup of an export ARN that
/// resolves to nothing with this one fixed string, whatever the ARN was — an ARN in the
/// caller's own account naming an export that was never created and an ARN naming a
/// DIFFERENT account get byte-identical replies, so the body confirms nothing about the
/// ARN the caller supplied. The message is therefore a constant and never interpolates
/// the request: the identity of the ARN that was not found is reported in the modelled
/// `ResourceId` member ("The identifier of the resource that was not found"), which is
/// where the model puts it.
pub const EXPORT_NOT_FOUND_MESSAGE: &str = "No matching definition found";

/// The `Message` a `ResourceNotFoundException` of the RESOURCE-TAGGING operations carries
/// (`ListTagsForResource`, `TagResource`, `UntagResource`).
///
/// Recorded AWS behavior: the tagging operations word the same condition as a resource
/// miss rather than as a missing export definition — the wire recording for a tag
/// listing of an absent export ARN carries this string where the definition operations
/// carry [`EXPORT_NOT_FOUND_MESSAGE`]. Like that one it is a fixed, request-independent
/// constant: which ARN was not found is reported in `ResourceId`.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The modelled `ResourceNotFoundException` for an export ARN that names nothing, worded
/// with `message`: [`EXPORT_NOT_FOUND_MESSAGE`] on the export-definition operations and
/// [`RESOURCE_NOT_FOUND_MESSAGE`] on the resource-tagging ones.
pub fn resource_not_found(arn: &str, message: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: message.to_string(),
        resource_id: arn.to_string(),
        resource_type: EXPORT_RESOURCE_TYPE.to_string(),
    }
}

/// The modelled `ResourceNotFoundException` an export-definition operation answers an
/// export ARN that names nothing with.
pub fn export_not_found(arn: &str) -> ResourceNotFoundException {
    resource_not_found(arn, EXPORT_NOT_FOUND_MESSAGE)
}

/// The HTTP status this service answers `ResourceNotFoundException` with.
///
/// The error SHAPE carries `@httpError 404`
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), but the SERVICE
/// answers the export-not-found condition with 400 carrying that same shape, its members
/// and its `__type`: the wire recordings for a read, an update, a delete and a tag
/// listing of an export ARN that resolves to nothing all answer HTTP 400. A client
/// branches on the status as well as on the code — and an HTTP-level proxy treats a 404
/// as a routing failure rather than an API error — so the pair it sees has to be the pair
/// the service sends. The status is stated here once and applied through
/// [`crate::scaffolded::Scaffolded::ModeledWithStatus`], which leaves the shape, its
/// members and its wire code the model's.
pub const EXPORT_NOT_FOUND_STATUS: u16 = 400;

/// [`export_not_found`] lifted into an operation's error channel, answered with
/// [`EXPORT_NOT_FOUND_STATUS`].
///
/// `wrap` is the calling operation's own `ResourceNotFoundException` variant, so every
/// operation that looks an export ARN up reports the miss through this ONE place and none
/// of them can drift back to the shape's own status.
pub fn export_not_found_error<E>(
    arn: &str,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    not_found_error(export_not_found(arn), wrap)
}

/// As [`export_not_found_error`], for the resource-tagging operations, which word the
/// condition with [`RESOURCE_NOT_FOUND_MESSAGE`].
pub fn tagged_resource_not_found_error<E>(
    arn: &str,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    not_found_error(resource_not_found(arn, RESOURCE_NOT_FOUND_MESSAGE), wrap)
}

/// `error` lifted into an operation's error channel at [`EXPORT_NOT_FOUND_STATUS`].
pub fn not_found_error<E>(
    error: ResourceNotFoundException,
    wrap: impl FnOnce(ResourceNotFoundException) -> E,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(wrap(error), EXPORT_NOT_FOUND_STATUS)
}

/// The modelled not-found for an `ExecutionId` that names no execution of the export it
/// was asked about.
///
/// It carries the same fixed [`EXPORT_NOT_FOUND_MESSAGE`] as every other lookup of this
/// service that resolves to nothing, for the reason stated there: the identity of what
/// was not found is reported in the modelled `ResourceId`, not in the prose.
pub fn execution_not_found(execution_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: EXPORT_NOT_FOUND_MESSAGE.to_string(),
        resource_id: execution_id.to_string(),
        resource_type: EXECUTION_RESOURCE_TYPE.to_string(),
    }
}

/// The message a tagging operation refuses a `ResourceArn` of another account with.
///
/// Recorded AWS behavior: an ARN that names a different account is refused by the tagging
/// operations with this sentence — which says nothing about the ARN, because the refusal
/// is reached BEFORE any lookup and so cannot report on a resource it never consulted.
/// (The recorded text is an example of the sentence; the requirement is the
/// `ValidationException` classification and that the refusal does not depend on
/// existence.)
pub const RESOURCE_NOT_TAGGABLE_MESSAGE: &str = "Resource doesn't support tagging";

/// The modelled 400 a tagging operation refuses an unusable `ResourceArn` with.
fn not_taggable() -> ValidationException {
    ValidationException {
        message: RESOURCE_NOT_TAGGABLE_MESSAGE.to_string(),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "ResourceArn".to_string(),
            message: "Must name a resource of this account.".to_string(),
        }]),
    }
}

/// Refuse `arn` unless it names a resource of THIS account.
///
/// AWS validates a tagging ARN's account before any existence lookup, so an ARN naming
/// another account is a `ValidationException` (400) rather than a
/// `ResourceNotFoundException`: no caller of this account can tag another account's
/// resource, whatever this account holds, and deciding it without a lookup is exactly
/// what keeps the answer from reporting on a resource AWS never consults — the
/// cross-tenant probe this ordering rules out.
///
/// What is left to check is only the ACCOUNT: the modelled `Arn` pattern
/// (`^arn:aws[-a-z0-9]*:(bcm-data-exports):[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`) is
/// already enforced at the boundary, so the partition, the service and the shape of the
/// account and resource segments hold by the time a handler runs. The RESOURCE segment is
/// deliberately not judged here: an ARN of this account whose resource names no export is
/// a resource this service looked for and did not find, which is the 404.
pub fn validate_taggable_resource_arn(arn: &str) -> Result<(), ValidationException> {
    let names_this_account = arn
        .split(':')
        .nth(ARN_ACCOUNT_SEGMENT)
        .is_some_and(|account| account == ::engine::emulator_account_id());
    if names_this_account {
        Ok(())
    } else {
        Err(not_taggable())
    }
}

/// The 0-based position of the account segment in an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
const ARN_ACCOUNT_SEGMENT: usize = 4;

/// The status a freshly created export carries.
pub fn fresh_status(at: ::aws_smithy_types::DateTime) -> ExportStatus {
    ExportStatus {
        status_code: Some(EXPORT_STATUS_HEALTHY.to_string()),
        status_reason: None,
        created_at: Some(at),
        last_updated_at: Some(at),
        // No export file has been generated yet: there is nothing to report a
        // last-refresh instant for until an execution delivers one.
        last_refreshed_at: None,
    }
}

/// Bytes of the integrity tag an opaque page token carries ahead of its position.
const PAGE_TOKEN_TAG_LEN: usize = 8;

/// Domain separator for the page-token tag, so a token minted for this service's listings
/// cannot be read as anything else.
const PAGE_TOKEN_DOMAIN: &[u8] = b"bcm-data-exports/page-token/v1\0";

/// Which SEQUENCE a page token names a position in: the listing that cut it and, where
/// that listing pages over the contents of one resource, which resource's contents.
///
/// A token is bound to that sequence as well as to its position, because the paginated
/// listings page over different key spaces (`ListExports` over export ARNs,
/// `ListTagsForResource` over one export's tag keys, `ListTables` over the table catalog's
/// names, `ListExecutions` over one export's execution ids): a token handed to a sequence
/// it was never cut from names a position in some other sequence — a tag key where an
/// execution id is expected, or a position in a DIFFERENT export's executions — and must
/// be refused rather than resumed at an unrelated offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenKind<'a> {
    /// `ListExports`, paging over the export ARNs in [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`].
    Exports,
    /// `ListTagsForResource`, paging over the tag keys of the export `resource_arn` names.
    ResourceTags {
        /// The export whose tags this token walks.
        resource_arn: &'a str,
    },
    /// `ListTables`, paging over the table names of [`TABLE_CATALOG`].
    Tables,
    /// `ListExecutions`, paging over the execution ids of the export `export_arn` names.
    Executions {
        /// The export whose executions this token walks.
        export_arn: &'a str,
    },
}

impl PageTokenKind<'_> {
    /// Mix the bytes that distinguish this sequence into the tag's preimage.
    ///
    /// A per-resource listing mixes the resource's ARN as well as the listing, and every
    /// field is NUL-terminated — an ARN of this service cannot contain a NUL (the modelled
    /// `Arn` pattern admits none), so no two sequences share a preimage.
    fn mix(self, hasher: &mut ::sha2::Sha256) {
        use ::sha2::Digest;
        match self {
            Self::Exports => hasher.update(b"Exports\0"),
            Self::ResourceTags { resource_arn } => {
                hasher.update(b"ResourceTags\0");
                hasher.update(resource_arn.as_bytes());
                hasher.update(b"\0");
            }
            Self::Tables => hasher.update(b"Tables\0"),
            Self::Executions { export_arn } => {
                hasher.update(b"Executions\0");
                hasher.update(export_arn.as_bytes());
                hasher.update(b"\0");
            }
        }
    }
}

/// The integrity tag of `key` for `kind`: the leading [`PAGE_TOKEN_TAG_LEN`] bytes of
/// SHA-256 over the service domain separator, the sequence, and the key.
fn page_token_tag(kind: PageTokenKind<'_>, key: &str) -> Vec<u8> {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(PAGE_TOKEN_DOMAIN);
    kind.mix(&mut hasher);
    hasher.update(key.as_bytes());
    hasher.finalize()[..PAGE_TOKEN_TAG_LEN].to_vec()
}

/// A page token naming the key the NEXT page of `kind` starts at: an integrity tag
/// followed by the key, hex-encoded. Opaque — a client cannot read a position out of it
/// and must hand it back verbatim — and self-verifying, so a token this listing never
/// issued is rejected instead of being read as some arbitrary position in it.
fn encode_page_token(kind: PageTokenKind<'_>, key: &str) -> String {
    let mut bytes = page_token_tag(kind, key);
    bytes.extend_from_slice(key.as_bytes());
    ::hex::encode(bytes)
}

/// The key a token issued by [`encode_page_token`] for `kind` names, or `None` when this
/// sequence never issued it: not hex, too short to carry a tag, not UTF-8, or carrying a
/// tag that does not match the sequence and position it claims.
fn decode_page_token(kind: PageTokenKind<'_>, token: &str) -> Option<String> {
    let bytes = ::hex::decode(token).ok()?;
    if bytes.len() < PAGE_TOKEN_TAG_LEN {
        return None;
    }
    let (tag, key) = bytes.split_at(PAGE_TOKEN_TAG_LEN);
    let key = String::from_utf8(key.to_vec()).ok()?;
    (page_token_tag(kind, &key) == tag).then_some(key)
}

/// One page of `items` (ordered by their keys): at most `max_results` values starting at
/// the key `token` names, plus the token that resumes immediately after the page.
///
/// `None` when `token` was not issued by the `kind` sequence — the caller answers that
/// with the modelled `ValidationException`. The returned token is `None` on the last
/// page, so a paginating client stops.
pub fn page<T: Clone>(
    kind: PageTokenKind<'_>,
    items: &[(String, T)],
    max_results: Option<i32>,
    token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match token {
        Some(token) if !token.is_empty() => {
            let key = decode_page_token(kind, token)?;
            items.partition_point(|(item_key, _)| item_key.as_str() < key.as_str())
        }
        _ => 0,
    };
    // An absent MaxResults means "no caller-imposed limit". The modelled range (1..300)
    // is already enforced at the boundary, so a present one is at least 1; the floor
    // keeps that true here independently of boundary validation, because a zero-sized
    // page handed out with a resume token would make a paginating client loop forever.
    let limit = max_results.map_or(items.len(), |max| max.max(1) as usize);
    let end = start.saturating_add(limit).min(items.len());
    let values = items[start..end]
        .iter()
        .map(|(_, value)| value.clone())
        .collect();
    let next = items
        .get(end)
        .map(|(key, _)| encode_page_token(kind, key));
    Some((values, next))
}

/// The most tags one export holds.
///
/// The modelled `ResourceTagList` is `@length 0..200` (smithy_query shape
/// `ResourceTagList`), and that list is both what a tagging request submits and what
/// `ListTagsForResource` reports for an export — so an export holding more than this many
/// tags could not be reported within its own modelled bound. A request that would take an
/// export past it is refused with the modelled `ValidationException` instead.
pub const MAX_RESOURCE_TAGS: usize = 200;

/// A submitted `ResourceTagList` collapsed into the keyed map the store holds, or the
/// modelled refusal for a list that names one key twice.
///
/// "Each tag consists of a key and a value, and each key must be unique for the resource"
/// (the modelled doc for `ResourceTags`), so a list carrying one key twice names two
/// values for one tag and cannot be satisfied. Lives here because every operation that
/// accepts a `ResourceTagList` — `CreateExport` and `TagResource` — must read it the same
/// way, or one of them would silently keep the last value for a duplicated key.
pub fn tag_map(tags: Vec<ResourceTag>) -> Result<BTreeMap<String, String>, ValidationException> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for tag in tags {
        if map.insert(tag.key.clone(), tag.value).is_some() {
            return Err(duplicate_tag_key(&tag.key));
        }
    }
    Ok(map)
}

/// The modelled 400 for a `ResourceTags` list that carries `key` more than once.
pub fn duplicate_tag_key(key: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "ResourceTags carries the key {} more than once; each tag key must be unique for the resource.",
            clip_for_message(key)
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "ResourceTags".to_string(),
            message: "Each tag key must be unique for the resource.".to_string(),
        }]),
    }
}

/// The modelled 400 for a request that would leave an export holding `total` tags, past
/// [`MAX_RESOURCE_TAGS`].
pub fn too_many_tags(total: usize) -> ValidationException {
    ValidationException {
        message: format!(
            "The request would leave the resource with {total} tags; a resource holds at most {MAX_RESOURCE_TAGS}."
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: "ResourceTags".to_string(),
            message: format!("A resource holds at most {MAX_RESOURCE_TAGS} tags."),
        }]),
    }
}

/// The tags of an export as the modelled list, ordered by key.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Vec<ResourceTag> {
    tags.iter()
        .map(|(key, value)| ResourceTag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// The `ValidationExceptionReason` a refusal of one named input member reports. One of
/// the modelled values (`crate::ops::types::VALIDATION_EXCEPTION_REASON_VALUES`:
/// `cannotParse`, `fieldValidationFailed`, `other`, `unknownOperation`).
pub const VALIDATION_REASON_FIELD_FAILED: &str = "fieldValidationFailed";

/// The most characters of a caller-supplied value a refusal message echoes.
///
/// `TableName` admits 1024 characters and a table property value 16384 (smithy_query
/// shapes `TableName`, `TableProperties`), while `ValidationException$Message` admits
/// only 1024 — so an echoed value is clipped to keep a refusal built from an outsized
/// one inside the message's own modelled bound.
const MESSAGE_VALUE_CLIP: usize = 128;

/// `value` as a refusal message quotes it: itself when short, else its first
/// [`MESSAGE_VALUE_CLIP`] characters and an ellipsis.
pub(crate) fn clip_for_message(value: &str) -> String {
    if value.chars().count() <= MESSAGE_VALUE_CLIP {
        return value.to_string();
    }
    let head: String = value.chars().take(MESSAGE_VALUE_CLIP).collect();
    format!("{head}...")
}

// The export's data query — its parsing, its bounds and its refusals — lives in
// [`query`], which `CreateExport` and `UpdateExport` both read through
// [`validate_data_query`]: a definition cannot be judged one way by one and another way
// by the other.
