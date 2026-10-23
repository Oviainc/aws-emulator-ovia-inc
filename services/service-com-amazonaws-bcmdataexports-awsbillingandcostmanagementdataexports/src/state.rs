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
//! operations read it.

use crate::ops::types::{
    Export, ExportStatus, ResourceNotFoundException, ResourceTag, ValidationException,
    ValidationExceptionField,
};
use std::collections::BTreeMap;

pub mod catalog;

// The catalog's types, data and refusals are reached as `crate::state::…` by the two
// operations that read it, exactly as they were before the catalog grew its own module.
pub use catalog::{
    table_spec, unknown_table, ColumnGroup, ColumnSpec, TablePropertySpec, TableSpec,
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

/// The current wall-clock instant, as the modelled `Timestamp` type.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now())
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
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("export/{name}"))
}

/// The `Message` every `ResourceNotFoundException` of this service carries.
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

/// The modelled 404 for an export ARN that names nothing.
pub fn export_not_found(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: EXPORT_NOT_FOUND_MESSAGE.to_string(),
        resource_id: arn.to_string(),
        resource_type: EXPORT_RESOURCE_TYPE.to_string(),
    }
}

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

/// Which listing a page token belongs to. A token is bound to its listing as well as to
/// its position, because the paginated listings page over different key spaces
/// (`ListExports` over export ARNs, `ListTagsForResource` over an export's tag keys,
/// `ListTables` over the table catalog's names): a token handed to the wrong listing
/// names a position in a sequence it was never cut from, and must be refused rather than
/// resumed at some unrelated offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageTokenKind {
    /// `ListExports`, paging over the export ARNs in [`ComAmazonawsBcmdataexportsAwsbillingandcostmanagementdataexportsState::exports`].
    Exports,
    /// `ListTagsForResource`, paging over one export's tag keys.
    ResourceTags,
    /// `ListTables`, paging over the table names of [`TABLE_CATALOG`].
    Tables,
}

impl PageTokenKind {
    /// The bytes that distinguish this listing inside the tag's preimage.
    fn domain(self) -> &'static [u8] {
        match self {
            Self::Exports => b"Exports\0",
            Self::ResourceTags => b"ResourceTags\0",
            Self::Tables => b"Tables\0",
        }
    }
}

/// The integrity tag of `key` for `kind`: the leading [`PAGE_TOKEN_TAG_LEN`] bytes of
/// SHA-256 over the service domain separator, the listing, and the key.
fn page_token_tag(kind: PageTokenKind, key: &str) -> Vec<u8> {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(PAGE_TOKEN_DOMAIN);
    hasher.update(kind.domain());
    hasher.update(key.as_bytes());
    hasher.finalize()[..PAGE_TOKEN_TAG_LEN].to_vec()
}

/// A page token naming the key the NEXT page of `kind` starts at: an integrity tag
/// followed by the key, hex-encoded. Opaque — a client cannot read a position out of it
/// and must hand it back verbatim — and self-verifying, so a token this listing never
/// issued is rejected instead of being read as some arbitrary position in it.
fn encode_page_token(kind: PageTokenKind, key: &str) -> String {
    let mut bytes = page_token_tag(kind, key);
    bytes.extend_from_slice(key.as_bytes());
    ::hex::encode(bytes)
}

/// The key a token issued by [`encode_page_token`] for `kind` names, or `None` when this
/// listing never issued it: not hex, too short to carry a tag, not UTF-8, or carrying a
/// tag that does not match the listing and position it claims.
fn decode_page_token(kind: PageTokenKind, token: &str) -> Option<String> {
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
/// `None` when `token` was not issued by the `kind` listing — the caller answers that
/// with the modelled `ValidationException`. The returned token is `None` on the last
/// page, so a paginating client stops.
pub fn page<T: Clone>(
    kind: PageTokenKind,
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
fn clip_for_message(value: &str) -> String {
    if value.chars().count() <= MESSAGE_VALUE_CLIP {
        return value.to_string();
    }
    let head: String = value.chars().take(MESSAGE_VALUE_CLIP).collect();
    format!("{head}...")
}
