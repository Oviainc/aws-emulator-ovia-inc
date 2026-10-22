// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigateway-backplanecontrolservice service.
//!
//! This holds the REST API resource tree the control plane persists — a `RestApi`, the
//! `Resource`s under it (including the implicit root resource every API is created
//! with) and the `Method`s put on those resources — beside the account's `ApiKey`
//! records. Every handler in `src/ops/` computes
//! its response from these records; nothing is synthesised per request.
//!
//! A few leaf values are stored as the modelled shapes from [`crate::ops::types`]
//! (`EndpointConfiguration`, `Method`'s responses and integration) rather than as a
//! second hand-written mirror of the same members: the model already declares them, and
//! two copies of one shape is how two operations end up reading two different records.

use std::collections::BTreeMap;

use crate::ops::types::{EndpointConfiguration, GetIntegrationOutput, GetMethodResponseOutput};

/// Per-service state for com-amazonaws-apigateway-backplanecontrolservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApigatewayBackplanecontrolserviceState {
    /// Every created `RestApi`, keyed by its API id.
    pub rest_apis: BTreeMap<String, RestApiEntry>,
    /// Every created `ApiKey`, keyed by its API key id — the identifier `GetApiKey`
    /// and `DeleteApiKey` take in their `apiKey` path label, NOT the key's value.
    pub api_keys: BTreeMap<String, ApiKeyEntry>,
    /// Drives [`ComAmazonawsApigatewayBackplanecontrolserviceState::mint_id`]; see there for how an id is derived from it.
    pub id_seed: u64,
    /// Drives [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`]: the creation rank stamped on each
    /// record so a listing can report creation order rather than id order.
    pub sequence_counter: u64,
}

/// One stored `RestApi`, with the resource tree that hangs off it.
#[derive(Debug, Clone, Default)]
pub struct RestApiEntry {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    pub binary_media_types: Vec<String>,
    pub minimum_compression_size: Option<i32>,
    pub api_key_source: Option<String>,
    pub endpoint_configuration: Option<EndpointConfiguration>,
    pub policy: Option<String>,
    pub security_policy: Option<String>,
    pub endpoint_access_mode: Option<String>,
    pub api_status: Option<String>,
    pub api_status_message: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub disable_execute_api_endpoint: bool,
    /// The id of this API's implicit root resource, which is also a key of `resources`.
    pub root_resource_id: String,
    /// Every resource of this API, keyed by resource id; the root resource included.
    pub resources: BTreeMap<String, ResourceEntry>,
    /// This API's creation rank, from [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`].
    pub sequence: u64,
}

impl RestApiEntry {
    /// This API's resources in creation order — the root resource first, because it is
    /// created with the API and so carries the lowest rank.
    pub fn resources_in_creation_order(&self) -> Vec<&ResourceEntry> {
        let mut ordered: Vec<&ResourceEntry> = self.resources.values().collect();
        ordered.sort_by_key(|resource| (resource.sequence, resource.id.clone()));
        ordered
    }

    /// The ids of [`Self::resources_in_creation_order`], the stable sequence
    /// [`page_of`] cuts a `GetResources` page out of.
    pub fn resource_ids_in_creation_order(&self) -> Vec<String> {
        self.resources_in_creation_order()
            .into_iter()
            .map(|resource| resource.id.clone())
            .collect()
    }
}

/// One stored `Resource`: a node of an API's path tree.
#[derive(Debug, Clone, Default)]
pub struct ResourceEntry {
    pub id: String,
    /// Absent on the root resource only.
    pub parent_id: Option<String>,
    /// Absent on the root resource only.
    pub path_part: Option<String>,
    /// The materialised full path, the parent's path with this resource's path part
    /// appended; `/` on the root resource.
    pub path: String,
    /// The methods put on this resource, keyed by uppercase HTTP verb.
    pub methods: BTreeMap<String, MethodEntry>,
    /// This resource's creation rank, from [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`]; `0` on
    /// the root resource.
    pub sequence: u64,
}

/// One stored `Method` on a resource.
#[derive(Debug, Clone, Default)]
pub struct MethodEntry {
    pub http_method: String,
    pub authorization_type: String,
    pub authorizer_id: Option<String>,
    pub api_key_required: bool,
    pub operation_name: Option<String>,
    pub request_parameters: BTreeMap<String, bool>,
    pub request_models: BTreeMap<String, String>,
    pub request_validator_id: Option<String>,
    pub authorization_scopes: Vec<String>,
    /// Written by `PutMethodResponse`, which is still a stub, so this stays empty for
    /// now and a `Method` body reports no `methodResponses`.
    pub method_responses: BTreeMap<String, GetMethodResponseOutput>,
    /// Written by `PutIntegration`, which is still a stub, so this stays absent for now
    /// and a `Method` body reports no `methodIntegration`.
    pub integration: Option<GetIntegrationOutput>,
}

/// One stored `ApiKey`: the identifier the control plane assigns, the key value a
/// client presents in `x-api-key`, and the record's own attributes.
#[derive(Debug, Clone, Default)]
pub struct ApiKeyEntry {
    pub id: String,
    /// The key's secret value. Minted by the emulator's credential store (see
    /// [`mint_api_key_value`]) or supplied by the caller through `CreateApiKey$value`,
    /// and reported only where the model says so — `CreateApiKey`, and `GetApiKey`
    /// when the request asks for it with `includeValue`.
    pub value: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub customer_id: Option<String>,
    pub enabled: bool,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    pub last_updated_date: Option<::aws_smithy_types::DateTime>,
    /// The stages this key is associated with, each rendered `<restApiId>/<stageName>`
    /// as the modelled `ApiKey$stageKeys` list of strings reports them.
    pub stage_keys: Vec<String>,
    pub tags: BTreeMap<String, String>,
    /// This key's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
}

/// The characters an API Gateway id is built from: lowercase alphanumerics.
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The length of an API Gateway `RestApi` / `Resource` id.
const ID_LEN: u32 = 10;

/// The number of distinct ids [`ComAmazonawsApigatewayBackplanecontrolserviceState::mint_id`] can render: `36^10`.
const ID_SPACE: u128 = 36u128.pow(ID_LEN);

/// Scrambles consecutive seeds across [`ID_SPACE`]. Prime, so it is coprime with
/// `36^10 = 2^20 * 3^20`, which makes `seed -> seed * MULTIPLIER + OFFSET (mod ID_SPACE)`
/// a BIJECTION: distinct seeds can never mint the same id.
const ID_MULTIPLIER: u128 = 1_000_000_007;

/// The bijection's constant term, so the first id minted is not a run of leading `a`s.
const ID_OFFSET: u128 = 734_291;

/// API Gateway's documented default page size: "The maximum number of returned results
/// per page. The default value is 25 and the maximum value is 500."
/// (`GetRestApisRequest$limit` and `GetResourcesRequest$limit` in this service's model.)
pub const DEFAULT_PAGE_LIMIT: i32 = 25;

/// API Gateway's documented maximum page size; see [`DEFAULT_PAGE_LIMIT`].
pub const MAX_PAGE_LIMIT: i32 = 500;

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit delete,
// so each growth site carries a cap and nothing caller-sized reaches the store
// unchecked. The caps are API Gateway's own published quotas where it publishes one;
// where the service publishes none, the constant says so and the value is this
// emulator's resource guard, set well above any realistic use.
//
// The product is therefore finite: at most MAX_REST_APIS APIs, each with at most
// MAX_RESOURCES_PER_API resources, each with at most one method per verb in the HTTP
// verb catalog, each method's caller-sized maps capped at MAX_COLLECTION_ENTRIES
// entries of at most MAX_STRING_CHARS characters.
// ---------------------------------------------------------------------------

/// API Gateway's default quota "APIs per account per Region: 600".
pub const MAX_REST_APIS: usize = 600;

/// API Gateway's default quota "API keys per account per Region: 10,000".
pub const MAX_API_KEYS: usize = 10_000;

/// API Gateway's default quota "Resources per API: 300".
pub const MAX_RESOURCES_PER_API: usize = 300;

/// API Gateway's documented resource-policy document size: 8192 characters.
pub const MAX_POLICY_CHARS: usize = 8192;

/// AWS's tagging limit of 50 tags per resource.
pub const MAX_TAGS: usize = 50;

/// From the `tags` member's own documentation: "The tag key can be up to 128
/// characters".
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// From the `tags` member's own documentation: "The tag value can be up to 256
/// characters."
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Emulator resource guard, NOT an AWS quota: the most entries this emulator will store
/// for a caller-sized list or map whose size the service publishes no limit for.
pub const MAX_COLLECTION_ENTRIES: usize = 50;

/// Emulator resource guard, NOT an AWS quota: the longest caller-supplied string this
/// emulator will store for a member whose length the service publishes no limit for.
pub const MAX_STRING_CHARS: usize = 512;

/// Emulator resource guard, NOT an AWS quota: the longest single path segment.
pub const MAX_PATH_PART_CHARS: usize = 128;

/// Emulator resource guard, NOT an AWS quota: the longest materialised resource path,
/// which also bounds how deeply resources may nest.
pub const MAX_PATH_CHARS: usize = 1024;

/// Refuse a caller-sized collection whose entry count would grow the store past `max`.
///
/// `Err` carries the message, naming the member and the bound it broke, for the caller
/// to fix the request from.
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

/// One page of an ordered id sequence, as a `position`/`limit` paginated collection
/// reports it.
#[derive(Debug, Clone)]
pub struct Page {
    /// The ids on this page, in the sequence's own order.
    pub ids: Vec<String>,
    /// The `position` cursor the NEXT page starts at; absent on the last page.
    pub next_position: Option<String>,
}

/// Cut the page a caller asked for out of `ordered`, a stable sequence of ids.
///
/// `position` is the id the page starts at — the cursor a previous page handed back —
/// and `None` starts at the first id. `limit` defaults to [`DEFAULT_PAGE_LIMIT`] and is
/// clamped into the documented `1..=`[`MAX_PAGE_LIMIT`] range.
///
/// `Err` carries the message for an unresolvable cursor: a `position` naming no id in
/// the sequence, which the caller must be told about rather than served an empty page.
pub fn page_of(
    ordered: &[String],
    position: Option<&str>,
    limit: Option<i32>,
) -> Result<Page, String> {
    let start = match position {
        None => 0,
        Some(cursor) => ordered
            .iter()
            .position(|id| id == cursor)
            .ok_or_else(|| format!("Invalid position: {cursor}"))?,
    };
    let size = limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
    let size = usize::try_from(size).unwrap_or(1);
    let end = start.saturating_add(size).min(ordered.len());
    Ok(Page {
        ids: ordered[start..end].to_vec(),
        next_position: ordered.get(end).cloned(),
    })
}

impl ComAmazonawsApigatewayBackplanecontrolserviceState {
    /// Mint a fresh id of the shape API Gateway assigns a `RestApi` or a `Resource`:
    /// ten lowercase alphanumeric characters.
    ///
    /// The id is derived from a counter through the bijection described on
    /// [`ID_MULTIPLIER`], so it is unpredictable-looking yet never repeats.
    pub fn mint_id(&mut self) -> String {
        self.id_seed = self.id_seed.wrapping_add(1);
        let mut scrambled =
            (u128::from(self.id_seed) * ID_MULTIPLIER + ID_OFFSET) % ID_SPACE;
        let mut id = Vec::with_capacity(ID_LEN as usize);
        for _ in 0..ID_LEN {
            let digit = usize::try_from(scrambled % 36).unwrap_or(0);
            id.push(ID_ALPHABET[digit]);
            scrambled /= 36;
        }
        String::from_utf8(id).unwrap_or_default()
    }

    /// The next creation rank. Stamped on a record when it is created, so a listing
    /// reports creation order even though the records are keyed by id.
    pub fn next_sequence(&mut self) -> u64 {
        self.sequence_counter = self.sequence_counter.wrapping_add(1);
        self.sequence_counter
    }

    /// Mint `count` ids at once, so a handler that needs several can take the store
    /// lock once and then do its work — building a cloned resource tree, say — with the
    /// lock released.
    pub fn reserve_ids(&mut self, count: usize) -> Vec<String> {
        (0..count).map(|_| self.mint_id()).collect()
    }

    /// The ids of every stored API in creation order, the stable sequence [`page_of`]
    /// cuts a `GetRestApis` page out of.
    pub fn rest_api_ids_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&RestApiEntry> = self.rest_apis.values().collect();
        ordered.sort_by_key(|api| (api.sequence, api.id.clone()));
        ordered.into_iter().map(|api| api.id.clone()).collect()
    }
}

/// The `NotFoundException` message for a `restApiId` that names no stored API. Shared
/// so every operation that resolves an API says the same thing about the same failure.
pub const NO_SUCH_REST_API: &str = "Invalid REST API identifier specified";

/// The `NotFoundException` message for a `resourceId` that names no stored resource.
pub const NO_SUCH_RESOURCE: &str = "Invalid Resource identifier specified";

/// The `NotFoundException` message for an `httpMethod` with no method on the resource.
pub const NO_SUCH_METHOD: &str = "Invalid Method identifier specified";

/// The `NotFoundException` message for an `apiKey` that names no stored API key.
pub const NO_SUCH_API_KEY: &str = "Invalid API Key identifier specified";

/// The `target` every audit record this service emits carries, so a consumer can select
/// the access trail without reading the rest of the emulator's traces.
///
/// A state-changing operation emits ONE record, at `INFO`, naming the operation, the
/// caller's presented access key id, the resource it changed and the outcome. Metadata
/// only — never a request body, a tag value or a secret, so an API key's `value` is
/// never among the fields.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// The identity a request presents, for an audit record: the access key id in its SigV4
/// credential scope. `emulator_server::extract_akid` answers `"default"` when the
/// request carries no scope. An access key id NAMES a caller and is not a secret — the
/// signature and the secret key are never read here.
pub fn request_caller(parts: &http::request::Parts) -> String {
    ::emulator_server::extract_akid(&parts.headers)
}

/// The length of an API Gateway-minted API key value: 40 characters, as observed from
/// AWS. A caller-supplied `CreateApiKey$value` is stored as given instead.
const API_KEY_VALUE_LEN: usize = 40;

/// A fresh API key value, for `CreateApiKey` when the request supplies none.
///
/// The value is a secret handed to a caller — a client presents it in `x-api-key` — but
/// it is NOT a credential of this emulator and must never be accepted as one. Nothing in
/// a service may construct a secret itself, so the value is cut from a secret minted by
/// the one component that generates secrets, the emulator's credential store, which is
/// then removed from that store again: the caller gets the secret, the emulator keeps no
/// credential it would accept, and nothing accumulates per call. Only the alphanumeric
/// characters of that secret are kept, because an API key value carries no others while
/// the store's secret alphabet also holds `+` and `/`.
///
/// `None` when the store minted no secret, or one with fewer than
/// [`API_KEY_VALUE_LEN`] usable characters, which leaves the operation to refuse rather
/// than report a value it made up.
pub fn mint_api_key_value(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let value: String = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(API_KEY_VALUE_LEN)
        .collect();
    // The key value is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    (value.chars().count() == API_KEY_VALUE_LEN).then_some(value)
}

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's 501
/// rather than a 5xx because there is nothing to retry: no engine of this process will
/// ever serve the call.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the com-amazonaws-apigateway-backplanecontrolservice emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApigatewayBackplanecontrolserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApigatewayBackplanecontrolserviceState>)
}
