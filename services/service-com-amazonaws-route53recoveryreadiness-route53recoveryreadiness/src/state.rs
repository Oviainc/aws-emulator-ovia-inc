// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53recoveryreadiness-route53recoveryreadiness service.
//!
//! Besides the state struct this module holds the few helpers the resource-set and
//! readiness-check handlers share — ARN construction, name validation, the
//! record-to-modelled-shape conversions and the list cursor codec — so the eight
//! operation files agree on one implementation of each instead of carrying eight
//! copies that can drift apart.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ops::types::{ReadinessCheckOutput, Resource, ResourceSetOutput};

/// The `arnNamespace` this service's ARNs carry, from the model's `aws.api#service`
/// trait (`smithy_query(query="service_info")`: `arnNamespace:
/// route53-recovery-readiness`).
pub const ARN_NAMESPACE: &str = "route53-recovery-readiness";

/// Route 53 Application Recovery Controller readiness is a GLOBAL service: its
/// readiness-check and resource-set ARNs carry an empty region segment
/// (`arn:aws:route53-recovery-readiness::<account>:resource-set/<name>`), which is
/// why no handler reads the request's region.
const ARN_REGION: &str = "";

/// The `@pattern` and `@length` every name this service reports carries, from the
/// `__stringMax64PatternAAZAZ09Z` shape the `ResourceSetName` / `ReadinessCheckName`
/// / `ResourceSet` OUTPUT members target
/// (`smithy_query(query="shape", shape="__stringMax64PatternAAZAZ09Z")`).
///
/// The matching INPUT members target plain `__string`, so the boundary validator
/// does not refuse a name the service could never report back; [`validate_name`]
/// is what applies the rule on the way in.
pub const NAME_PATTERN: &str = r"^\A[a-zA-Z0-9_]+\z$";

/// The maximum name length the same shape declares (`length 0..64`).
pub const NAME_MAX_LENGTH: usize = 64;

/// The bounds `MaxResults` declares on both list operations (`range 1..1000`,
/// `smithy_query(query="shape", shape="MaxResults")`).
pub const MAX_RESULTS_MIN: i32 = 1;
pub const MAX_RESULTS_MAX: i32 = 1000;

/// The page size a list uses when the caller sends no `MaxResults`.
///
/// The model publishes no default page size, so this is the emulator's own: a caller
/// who names no page size gets a bounded page and a `NextToken`, rather than having a
/// one-line request amplified into every record the store holds. A caller who wants
/// more asks for it, up to [`MAX_RESULTS_MAX`].
pub const DEFAULT_MAX_RESULTS: usize = 100;

/// The maximum number of records this emulator keeps in EACH of its caller-driven
/// stores (resource sets, readiness checks, cross-account authorizations).
///
/// This is NOT an AWS service quota — Route 53 Application Recovery Controller's
/// published limits are per-account and adjustable, and nothing here claims to
/// reproduce them. It is the emulator's own memory ceiling: every successful create
/// inserts a record that lives until a caller deletes it, so without a ceiling a
/// caller varying the name could grow the process until the host runs out of memory.
pub const MAX_RECORDS_PER_STORE: usize = 1_000;

/// The maximum number of `Resource` entries one resource set may carry, and the
/// maximum number of `ReadinessScopes` one of those entries may carry.
///
/// Also the emulator's own ceilings rather than AWS quotas: together with
/// [`MAX_RECORDS_PER_STORE`] and [`MAX_TAGS_PER_RESOURCE`] they make the total a store
/// can hold — and therefore the largest list response — a finite product instead of
/// something a caller chooses.
pub const MAX_RESOURCES_PER_SET: usize = 100;
pub const MAX_READINESS_SCOPES_PER_RESOURCE: usize = 50;

/// The maximum number of tags one record may carry (the emulator's own ceiling).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// Longest prefix of a rejected value a validation message echoes back, so a
/// multi-megabyte member cannot be amplified into a multi-megabyte error body.
const ECHO_LIMIT: usize = 64;

/// The message every handler answers with when the engine it was handed is not the
/// one this service's store lives in. One wording in one place, rather than eight
/// copies that can drift.
pub const STORE_UNAVAILABLE: &str =
    "the Route 53 Application Recovery Controller readiness store is unavailable";

/// One stored resource set: everything `CreateResourceSet` was given plus the ARN
/// the service minted for it, so `GetResourceSet` and `ListResourceSets` report the
/// identical values `CreateResourceSet` returned instead of recomputing them.
///
/// `resources` holds the modelled [`Resource`] shape itself rather than a parallel
/// record type: the shape is `Clone` + `Debug`, every member of it (including the
/// whole `DnsTargetResource` subtree) is preserved exactly as submitted, and one
/// representation cannot drift from the other.
#[derive(Debug, Clone)]
pub struct ResourceSetRecord {
    pub arn: String,
    pub name: String,
    pub resource_set_type: String,
    pub resources: Vec<Resource>,
    pub tags: BTreeMap<String, String>,
}

/// One stored readiness check: the ARN the service minted, the name it is keyed by,
/// the resource set it watches and its tags.
#[derive(Debug, Clone)]
pub struct ReadinessCheckRecord {
    pub arn: String,
    pub name: String,
    pub resource_set_name: String,
    pub tags: BTreeMap<String, String>,
}

/// Which record an ARN belongs to, so an ARN-addressed lookup resolves back to the
/// owning map. Readiness and architecture resources this slice does not store yet
/// (cells, recovery groups) get their own variants when they are implemented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceKey {
    ResourceSet(String),
    ReadinessCheck(String),
}

impl ResourceSetRecord {
    /// The persisted tags as the modelled `Tags` map, or `None` when the resource
    /// carries none: an untagged resource reports no `tags` member rather than an
    /// empty object.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        modeled_tags(&self.tags)
    }

    /// This record as a `ListResourceSets` entry.
    pub fn to_resource_set_output(&self) -> ResourceSetOutput {
        ResourceSetOutput {
            resource_set_arn: self.arn.clone(),
            resource_set_name: self.name.clone(),
            resource_set_type: self.resource_set_type.clone(),
            resources: self.resources.clone(),
            tags: self.modeled_tags(),
        }
    }
}

impl ReadinessCheckRecord {
    /// The persisted tags as the modelled `Tags` map, or `None` when the resource
    /// carries none.
    pub fn modeled_tags(&self) -> Option<HashMap<String, String>> {
        modeled_tags(&self.tags)
    }

    /// This record as a `ListReadinessChecks` entry.
    pub fn to_readiness_check_output(&self) -> ReadinessCheckOutput {
        ReadinessCheckOutput {
            readiness_check_arn: self.arn.clone(),
            readiness_check_name: Some(self.name.clone()),
            resource_set: self.resource_set_name.clone(),
            tags: self.modeled_tags(),
        }
    }
}

fn modeled_tags(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        return None;
    }
    Some(tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// Per-service state for com-amazonaws-route53recoveryreadiness-route53recoveryreadiness. A handler reaches it through [`service_state`].
///
/// `BTreeMap` everywhere, so list order — and therefore pagination — is the same on
/// every call and a cursor can be a key rather than an index.
///
/// GROWTH IS BOUNDED. Each store holds at most [`MAX_RECORDS_PER_STORE`] records, each
/// record at most [`MAX_RESOURCES_PER_SET`] resources ([`MAX_READINESS_SCOPES_PER_RESOURCE`]
/// scopes each) and [`MAX_TAGS_PER_RESOURCE`] tags, and each list response at most
/// [`MAX_RESULTS_MAX`] records — [`DEFAULT_MAX_RESULTS`] when the caller names no page
/// size — so both the retained state and the largest response are a finite product of
/// those ceilings rather than something a caller chooses. The size of the individual
/// strings inside a record is bounded by the server's own request-size limit.
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState {
    /// Every stored resource set, keyed by `ResourceSetName` (this API's identity
    /// for the resource).
    pub resource_sets: BTreeMap<String, ResourceSetRecord>,
    /// Every stored readiness check, keyed by `ReadinessCheckName`.
    pub readiness_checks: BTreeMap<String, ReadinessCheckRecord>,
    /// ARN -> owning record, so an ARN-addressed call (the tag operations, which
    /// take a `ResourceArn` rather than a name) resolves one without scanning
    /// every map.
    pub arn_index: BTreeMap<String, ResourceKey>,
    /// Every cross-account readiness authorization this account has in place.
    ///
    /// A SET, because the authorization value is the resource's whole identity (the
    /// API carries no other member for it, and `ListCrossAccountAuthorizations`
    /// reports a bare list of those values): the same value cannot be authorized
    /// twice, and `BTreeSet` keeps list order — and therefore pagination — stable.
    pub cross_account_authorizations: BTreeSet<String>,
}

/// The ARN of the resource set named `name`:
/// `arn:aws:route53-recovery-readiness::<account>:resource-set/<name>`.
pub fn resource_set_arn(name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, ARN_REGION, &format!("resource-set/{name}"))
}

/// The ARN of the readiness check named `name`:
/// `arn:aws:route53-recovery-readiness::<account>:readiness-check/<name>`.
pub fn readiness_check_arn(name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        ARN_REGION,
        &format!("readiness-check/{name}"),
    )
}

/// True when `value` satisfies [`NAME_PATTERN`].
///
/// The pattern is `^\A[a-zA-Z0-9_]+\z$`: `\A` and `\z` are the same anchors as `^`
/// and `$` on a single-line value, so it admits exactly a non-empty run of ASCII
/// letters, digits and underscores and nothing else. That character class is what this
/// checks directly — a one-line class, not a language, so it needs no regex engine, and
/// the rejected value is reported against the pattern literal itself so the message and
/// the check cannot disagree.
fn matches_name_pattern(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Refuse a name the service could not report back, in the wording AWS's boundary
/// validator uses: the rejected value, the member path and the constraint it broke.
///
/// `path` is the member's own name (`ResourceSetName`, `ReadinessCheckName`,
/// `ResourceSetName` on a create's resource-set reference), so the caller can fix
/// the request from the message alone.
pub fn validate_name(path: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > NAME_MAX_LENGTH {
        return Err(validation_message(
            path,
            value,
            &format!("Member must have length less than or equal to {NAME_MAX_LENGTH}"),
        ));
    }
    if !matches_name_pattern(value) {
        return Err(validation_message(
            path,
            value,
            &format!("Member must satisfy regular expression pattern: {NAME_PATTERN}"),
        ));
    }
    Ok(())
}

/// Refuse a create because the store is already at [`MAX_RECORDS_PER_STORE`]; `store`
/// names the records it holds, e.g. `"resource sets"`.
///
/// Answered as a 400 `ValidationException` rather than as the 429 `ThrottlingException`
/// the model also declares: neither create models a limit error, and a 429 is
/// retryable, so an SDK would spin on a refusal that cannot clear until a caller
/// deletes something.
pub fn store_full_message(store: &str) -> String {
    format!(
        "this emulator holds at most {MAX_RECORDS_PER_STORE} {store}; \
         delete one before creating another"
    )
}

/// Refuse a cross-account authorization value the service could never address back.
///
/// The model constrains `CrossAccountAuthorization` with neither `@length` nor
/// `@pattern` (`smithy_query(query="shape", shape="CrossAccountAuthorization")`), so
/// the boundary validator accepts any string and this is the only rule applied: the
/// value is the resource's identity and `DeleteCrossAccountAuthorization` binds it to
/// an `@httpLabel` (`DELETE /crossaccountauthorizations/{CrossAccountAuthorization}`),
/// which Smithy forbids from being empty — an empty value would name a resource no
/// later call could delete.
pub fn validate_cross_account_authorization(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(validation_message(
            "crossAccountAuthorization",
            value,
            "Member must have length greater than or equal to 1",
        ));
    }
    Ok(())
}

/// Refuse a resource list that is longer than this emulator stores, or one whose entry
/// carries more readiness scopes than it stores.
pub fn validate_resources(resources: &[Resource]) -> Result<(), String> {
    if resources.len() > MAX_RESOURCES_PER_SET {
        return Err(count_message(
            "Resources",
            resources.len(),
            MAX_RESOURCES_PER_SET,
        ));
    }
    for (index, resource) in resources.iter().enumerate() {
        let scopes = resource.readiness_scopes.as_ref().map_or(0, Vec::len);
        if scopes > MAX_READINESS_SCOPES_PER_RESOURCE {
            return Err(count_message(
                &format!("Resources.{}.ReadinessScopes", index + 1),
                scopes,
                MAX_READINESS_SCOPES_PER_RESOURCE,
            ));
        }
    }
    Ok(())
}

/// Refuse a tag map with more entries than this emulator stores per record.
pub fn validate_tags(tags: Option<&HashMap<String, String>>) -> Result<(), String> {
    let count = tags.map_or(0, HashMap::len);
    if count > MAX_TAGS_PER_RESOURCE {
        return Err(count_message("Tags", count, MAX_TAGS_PER_RESOURCE));
    }
    Ok(())
}

/// One validation clause for a collection that is too large. The COUNT is reported
/// rather than the collection, so a refusal cannot amplify an oversized request into
/// an oversized error body.
fn count_message(path: &str, count: usize, max: usize) -> String {
    format!(
        "1 validation error detected: Value with {count} entries at '{path}' failed to \
         satisfy constraint: Member must have length less than or equal to {max}"
    )
}

/// One validation clause in the envelope AWS uses.
fn validation_message(path: &str, value: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{}' at '{path}' failed to satisfy constraint: {requirement}",
        echoed(value)
    )
}

/// `value`, truncated on a character boundary to [`ECHO_LIMIT`] characters.
fn echoed(value: &str) -> String {
    if value.chars().count() <= ECHO_LIMIT {
        return value.to_string();
    }
    value.chars().take(ECHO_LIMIT).collect::<String>() + "..."
}

/// The tag every cursor this service mints carries ahead of the key it points at, so a
/// token the service did not mint is refused rather than honoured as a start key.
const CURSOR_TAG: &str = "v1:";

/// The pagination cursor for a page that starts at `key`.
///
/// Hex of a tagged payload rather than the key itself, so a caller cannot read
/// structure out of the token: a `NextToken` is opaque and only ever travels back
/// unchanged.
pub fn encode_cursor(key: &str) -> String {
    ::hex::encode(format!("{CURSOR_TAG}{key}").as_bytes())
}

/// The inclusive start key a cursor minted by [`encode_cursor`] carries, or `None`
/// when the token was not minted by this service — not hex, not text, or not carrying
/// [`CURSOR_TAG`].
pub fn decode_cursor(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(CURSOR_TAG).map(str::to_string)
}

/// One page of a list operation: the records on it, and the cursor the next call
/// resumes from (`None` on the last page).
#[derive(Debug)]
pub struct Page<'a, V> {
    pub records: Vec<&'a V>,
    pub next_token: Option<String>,
}

/// The page of `records` the caller asked for.
///
/// `max_results` bounds the page (defaulting to [`DEFAULT_MAX_RESULTS`]) and
/// `next_token` resumes at the key the previous page stopped before. `Err` carries a
/// validation message for a token this service did not mint.
pub fn page<'a, V>(
    records: &'a BTreeMap<String, V>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<'a, V>, String> {
    let start = start_key(next_token)?;
    // `range` from the cursor's key rather than a filtered full scan, so following a
    // cursor to the end of the store costs one walk of it and not one per page. The
    // bound borrows as `&str`, so forming it allocates nothing.
    let from = start.as_deref().unwrap_or("");
    Ok(take_page(
        records
            .range::<str, _>((
                ::std::ops::Bound::Included(from),
                ::std::ops::Bound::Unbounded,
            ))
            .map(|(key, record)| (key.as_str(), record)),
        page_size(max_results),
    ))
}

/// The page of `values` the caller asked for, for a store whose record IS its key — a
/// set of identity-only resources such as the cross-account authorizations.
///
/// Same page size, cursor codec and refusal as [`page`], so the two list surfaces
/// cannot drift apart.
pub fn page_set<'a>(
    values: &'a BTreeSet<String>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<'a, String>, String> {
    let start = start_key(next_token)?;
    let from = start.as_deref().unwrap_or("");
    Ok(take_page(
        values
            .range::<str, _>((
                ::std::ops::Bound::Included(from),
                ::std::ops::Bound::Unbounded,
            ))
            .map(|value| (value.as_str(), value)),
        page_size(max_results),
    ))
}

/// How many records one page carries: what the caller asked for, or
/// [`DEFAULT_MAX_RESULTS`].
fn page_size(max_results: Option<i32>) -> usize {
    match max_results {
        // PURELY DEFENSIVE: `MaxResults` carries the model's `range 1..1000`, so the
        // boundary validator has already refused anything outside it and this clamp
        // cannot fire. It is here so a page size can never be zero (which would make
        // the cursor non-terminating) if the member's constraint ever changes.
        Some(n) => n.clamp(MAX_RESULTS_MIN, MAX_RESULTS_MAX) as usize,
        None => DEFAULT_MAX_RESULTS,
    }
}

/// The inclusive key the page starts at, or `None` for the first page. `Err` carries a
/// validation message for a token this service did not mint.
fn start_key(next_token: Option<&str>) -> Result<Option<String>, String> {
    match next_token {
        Some(token) => Ok(Some(decode_cursor(token).ok_or_else(|| {
            validation_message(
                "nextToken",
                token,
                "Member must be a pagination token returned by a previous call",
            )
        })?)),
        None => Ok(None),
    }
}

/// One page off an ordered `(key, record)` walk that already starts at the cursor: the
/// first `page_size` records, and the cursor pointing at the record after them.
fn take_page<'a, V>(
    entries: impl Iterator<Item = (&'a str, &'a V)>,
    page_size: usize,
) -> Page<'a, V> {
    let mut page = Page {
        records: Vec::new(),
        next_token: None,
    };
    for (key, record) in entries {
        if page.records.len() == page_size {
            page.next_token = Some(encode_cursor(key));
            break;
        }
        page.records.push(record);
    }
    page
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53recoveryreadinessRoute53recoveryreadinessState>)
}
