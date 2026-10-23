// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigateway-backplanecontrolservice service.
//!
//! This holds the REST API resource tree the control plane persists — a `RestApi`, the
//! `Resource`s under it (including the implicit root resource every API is created
//! with), the `Method`s put on those resources and the API's `Stage`s — beside the
//! account's `ApiKey` records. Every handler in `src/ops/` computes
//! its response from these records; nothing is synthesised per request.
//!
//! A few leaf values are stored as the modelled shapes from [`crate::ops::types`]
//! (`EndpointConfiguration`, `Method`'s responses and integration) rather than as a
//! second hand-written mirror of the same members: the model already declares them, and
//! two copies of one shape is how two operations end up reading two different records.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    CanarySettings, DocumentationPartLocation, EndpointConfiguration, GetIntegrationOutput,
    GetMethodResponseOutput, MethodSetting, MethodSnapshot,
};

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
    /// What the request set, or [`DEFAULT_API_KEY_SOURCE`] when it set nothing: the
    /// service picks a value at create time, so a stored API always has one.
    pub api_key_source: Option<String>,
    /// What the request set, completed by [`defaulted_endpoint_configuration`], so the
    /// stored configuration always names its endpoint types and its IP address type.
    pub endpoint_configuration: Option<EndpointConfiguration>,
    pub policy: Option<String>,
    /// What the request set, or [`DEFAULT_SECURITY_POLICY`]; see [`api_key_source`].
    ///
    /// [`api_key_source`]: RestApiEntry::api_key_source
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
    /// Every `Stage` of this API, keyed by stage name. A stage is what an API key is
    /// associated with and what a caller invokes, so it is the record a `stageKeys`
    /// entry must name.
    pub stages: BTreeMap<String, StageEntry>,
    /// Every `Deployment` of this API, keyed by deployment id: what a stage points at.
    pub deployments: BTreeMap<String, DeploymentEntry>,
    /// Every `DocumentationPart` of this API, keyed by documentation part id: the
    /// records `CreateDocumentationPart` and `ImportDocumentationParts` write and
    /// `GetDocumentationPart` / `GetDocumentationParts` report.
    pub documentation_parts: BTreeMap<String, DocumentationPartEntry>,
    /// Every `Authorizer` of this API, keyed by authorizer id — the identifier a
    /// `Method`'s `authorizerId` names and `GetAuthorizer` takes in its path label.
    pub authorizers: BTreeMap<String, AuthorizerEntry>,
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

    /// Every resource of this API that `resource_id` owns — itself and everything whose
    /// stored `parentId` chain reaches it.
    ///
    /// This is the subtree a resource carries with it: the set `DeleteResource` removes,
    /// the set whose derived `path` a rename or a move recomputes, and the set a resource
    /// may not be moved into. One walk, so the ownership rule cannot differ between the
    /// operations that apply it. A record whose parent link is missing ends its branch.
    pub fn descendant_ids(&self, resource_id: &str) -> Vec<String> {
        let mut found: BTreeSet<String> = BTreeSet::new();
        found.insert(resource_id.to_string());
        let mut frontier = vec![resource_id.to_string()];
        while let Some(parent) = frontier.pop() {
            for child in self.resources.values() {
                if child.parent_id.as_deref() == Some(parent.as_str())
                    && found.insert(child.id.clone())
                {
                    frontier.push(child.id.clone());
                }
            }
        }
        found.into_iter().collect()
    }

    /// This API's documentation parts in creation order — the stable sequence
    /// [`page_of`] cuts a `GetDocumentationParts` page out of.
    pub fn documentation_parts_in_creation_order(&self) -> Vec<&DocumentationPartEntry> {
        let mut ordered: Vec<&DocumentationPartEntry> = self.documentation_parts.values().collect();
        ordered.sort_by_key(|part| (part.sequence, part.id.clone()));
        ordered
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

/// One stored `Stage` of an API: a named pointer at one deployment, which is what a
/// caller invokes and what an `ApiKey`'s `stageKeys` entry associates a key with.
#[derive(Debug, Clone, Default)]
pub struct StageEntry {
    pub stage_name: String,
    /// The deployment this stage serves; always the id of a stored
    /// [`DeploymentEntry`] of the same API.
    pub deployment_id: String,
    pub description: Option<String>,
    pub cache_cluster_enabled: bool,
    pub cache_cluster_size: Option<String>,
    /// When the stage's cache cluster finishes being created. A cluster is reported
    /// `CREATE_IN_PROGRESS` until this instant and `AVAILABLE` after it; absent when the
    /// stage has no cache cluster, which is reported `NOT_AVAILABLE`. See
    /// [`CACHE_CLUSTER_CREATE_SPAN`].
    pub cache_cluster_ready_at: Option<::std::time::Instant>,
    pub variables: BTreeMap<String, String>,
    pub documentation_version: Option<String>,
    pub canary_settings: Option<CanarySettings>,
    pub tracing_enabled: bool,
    /// Written by `UpdateStage`, which is still a stub, so this stays empty for now and
    /// a `Stage` body reports an empty `methodSettings` map.
    pub method_settings: BTreeMap<String, MethodSetting>,
    pub tags: BTreeMap<String, String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    pub last_updated_date: Option<::aws_smithy_types::DateTime>,
}

impl StageEntry {
    /// The `cacheClusterStatus` this stage reports now.
    ///
    /// A stage without a cache cluster has none to report, which is `NOT_AVAILABLE`. A
    /// cluster the caller asked for is created over [`CACHE_CLUSTER_CREATE_SPAN`], so a
    /// caller reading straight after the call still sees `CREATE_IN_PROGRESS`; the
    /// transition is one the CLOCK owes, materialised from the stored due instant rather
    /// than by sleeping or by being read.
    pub fn cache_cluster_status(&self) -> &'static str {
        match self.cache_cluster_ready_at {
            None => CACHE_CLUSTER_NOT_AVAILABLE,
            Some(ready_at) if ::std::time::Instant::now() < ready_at => {
                CACHE_CLUSTER_CREATE_IN_PROGRESS
            }
            Some(_) => CACHE_CLUSTER_AVAILABLE,
        }
    }
}

/// One stored `Deployment` of an API: the snapshot of the API's methods at the instant
/// it was created, which a `Stage` points at.
#[derive(Debug, Clone, Default)]
pub struct DeploymentEntry {
    pub id: String,
    pub description: Option<String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    /// The `apiSummary` this deployment reports: resource path -> HTTP verb -> the
    /// method as it stood when the deployment was created.
    pub api_summary: BTreeMap<String, BTreeMap<String, MethodSnapshot>>,
    /// This deployment's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`].
    pub sequence: u64,
}

/// One stored `DocumentationPart`: the API entity the documentation applies to, and the
/// content map describing it.
///
/// `location` is kept as the modelled [`DocumentationPartLocation`] rather than a second
/// hand-written mirror of its five members, for the reason the module docs give — and it
/// is stored COMPLETED (the members the entity type makes valid, defaulted where the
/// model documents a default), so every read of the part reports the same location.
#[derive(Debug, Clone, Default)]
pub struct DocumentationPartEntry {
    pub id: String,
    pub location: DocumentationPartLocation,
    /// The content map, as the JSON text the request supplied: "The map must be encoded
    /// as a JSON string" (`DocumentationPart$properties`).
    pub properties: String,
    /// This part's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing reports creation order rather than id order.
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
    /// Written by `PutMethodResponse` and reported by `GetMethodResponse` and a `Method`
    /// body's `methodResponses`, keyed by the response's `statusCode`; empty until a
    /// response is put on the method.
    pub method_responses: BTreeMap<String, GetMethodResponseOutput>,
    /// Written by `PutIntegration` and reported by `GetIntegration` and a `Method`
    /// body's `methodIntegration`; absent until an integration is put on the method.
    pub integration: Option<GetIntegrationOutput>,
}

/// One stored `Authorizer` of an API: the custom authorization an API's methods may
/// name in their `authorizerId`.
///
/// `authorizer_result_ttl_in_seconds` is NOT optional here even though the request
/// member is: the service picks [`DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS`] when the
/// request sets nothing, so a stored authorizer always has a TTL. Everything else is
/// kept exactly as the request set it, so a reader reports what was stored rather than a
/// value invented per read.
#[derive(Debug, Clone, Default)]
pub struct AuthorizerEntry {
    pub id: String,
    pub name: String,
    /// One of [`crate::ops::types::AUTHORIZER_TYPE_VALUES`]; the boundary validator
    /// refuses anything else before a handler runs.
    pub r#type: String,
    pub provider_arns: Vec<String>,
    pub auth_type: Option<String>,
    pub authorizer_uri: Option<String>,
    pub authorizer_credentials: Option<String>,
    pub identity_source: Option<String>,
    pub identity_validation_expression: Option<String>,
    /// The TTL in seconds of cached authorizer results; `0` disables caching.
    pub authorizer_result_ttl_in_seconds: i32,
    /// This authorizer's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
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
    /// The labels an `UpdateApiKey` patch of `/labels` adds to and removes from the key.
    ///
    /// Stored and reported by nothing: AWS accepts `/labels` as a collection path on an
    /// API key (a recorded refusal lists it beside `/stages`), while the `ApiKey` shape
    /// this service's model declares carries no member for it — so the patch is applied
    /// to the record and no response renders it.
    pub labels: Vec<String>,
    pub tags: BTreeMap<String, String>,
    /// This key's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
}

// ---------------------------------------------------------------------------
// Service-side defaults for a created `RestApi`
//
// Several `RestApi` members are optional on input and yet never absent on a created
// API: the service picks a value when the request sets none, stores it, and reports it
// from every read. The constants below are those values, and
// [`defaulted_endpoint_configuration`] is the one place the nested
// `EndpointConfiguration` is completed, so a record can never be stored half-defaulted.
// ---------------------------------------------------------------------------

/// The `apiKeySource` a created API carries when the request sets none: from the
/// member's own documentation, `HEADER` reads the key from the `X-API-Key` header, which
/// is where API Gateway looks unless a custom authorizer is named instead.
pub const DEFAULT_API_KEY_SOURCE: &str = "HEADER";

/// The `endpointConfiguration.types` entry a created API carries when the request sets
/// none: an edge-optimized API, which is API Gateway's default endpoint type (the
/// `types` member documents `EDGE` as "an edge-optimized API").
pub const DEFAULT_ENDPOINT_TYPE: &str = "EDGE";

/// The endpoint type whose only supported IP address type is [`DUALSTACK_ADDRESS_TYPE`];
/// from `EndpointConfiguration$ipAddressType`: "For the `PRIVATE` endpoint type, only
/// `dualstack` is supported."
pub const PRIVATE_ENDPOINT_TYPE: &str = "PRIVATE";

/// The `endpointConfiguration.ipAddressType` a created API carries when the request
/// sets none and the API is not private: `ipv4`, the first of the two documented values
/// ("Use `ipv4` to allow only IPv4 addresses to invoke an API").
pub const IPV4_ADDRESS_TYPE: &str = "ipv4";

/// The `ipAddressType` a private API carries when the request sets none; see
/// [`PRIVATE_ENDPOINT_TYPE`].
pub const DUALSTACK_ADDRESS_TYPE: &str = "dualstack";

/// The `securityPolicy` a created API carries when the request sets none: `TLS_1_0`, the
/// TLS version API Gateway starts an API on (and the first value of the modelled
/// `SecurityPolicy` enum).
pub const DEFAULT_SECURITY_POLICY: &str = "TLS_1_0";

/// Complete a request's (possibly absent) `endpointConfiguration` with the values the
/// service fills in, so what is stored is what every read of the API must report.
///
/// An absent or empty `types` becomes [`DEFAULT_ENDPOINT_TYPE`]; an absent
/// `ipAddressType` becomes [`DUALSTACK_ADDRESS_TYPE`] for a private API, which supports
/// no other, and [`IPV4_ADDRESS_TYPE`] otherwise. Whatever the request DID set — its
/// endpoint types, its address type, its `vpcEndpointIds` — is kept as given.
pub fn defaulted_endpoint_configuration(
    requested: Option<EndpointConfiguration>,
) -> EndpointConfiguration {
    let mut configuration = requested.unwrap_or_default();
    let types = match configuration.types {
        Some(types) if !types.is_empty() => types,
        _ => vec![DEFAULT_ENDPOINT_TYPE.to_string()],
    };
    let private = types.iter().any(|kind| kind == PRIVATE_ENDPOINT_TYPE);
    configuration.ip_address_type.get_or_insert_with(|| {
        if private {
            DUALSTACK_ADDRESS_TYPE.to_string()
        } else {
            IPV4_ADDRESS_TYPE.to_string()
        }
    });
    configuration.types = Some(types);
    configuration
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

/// API Gateway's default quota "Documentation parts: 2000 per API".
pub const MAX_DOCUMENTATION_PARTS_PER_API: usize = 2_000;

/// API Gateway's default quota "Stages per API: 10".
pub const MAX_STAGES_PER_API: usize = 10;

/// API Gateway's default quota "Authorizers per API: 10".
pub const MAX_AUTHORIZERS_PER_API: usize = 10;

/// Emulator resource guard, NOT an AWS quota: the most deployments this emulator keeps
/// for one API. API Gateway publishes no deployment quota.
pub const MAX_DEPLOYMENTS_PER_API: usize = 300;

/// From `CreateStage$stageName`'s own documentation: "Maximum length is 128
/// characters."
pub const MAX_STAGE_NAME_CHARS: usize = 128;

/// How long a stage's cache cluster takes to come up, after which it reports
/// `AVAILABLE`. API Gateway documents no duration for it ("it can take up to 4 minutes
/// to create" is guidance, not a contract), so this span is the emulator's own, chosen
/// so a caller reading straight after `CreateStage` still observes
/// `CREATE_IN_PROGRESS`.
pub const CACHE_CLUSTER_CREATE_SPAN: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

/// The `cacheClusterStatus` of a stage with no cache cluster.
pub const CACHE_CLUSTER_NOT_AVAILABLE: &str = "NOT_AVAILABLE";

/// The `cacheClusterStatus` of a cache cluster that is still coming up.
pub const CACHE_CLUSTER_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The `cacheClusterStatus` of a cache cluster that is up.
pub const CACHE_CLUSTER_AVAILABLE: &str = "AVAILABLE";

/// From `Authorizer$authorizerResultTtlInSeconds`'s own documentation: "If this field
/// is not set, the default value is 300."
pub const DEFAULT_AUTHORIZER_RESULT_TTL_SECONDS: i32 = 300;

/// From `Authorizer$authorizerResultTtlInSeconds`'s own documentation: "If it equals 0,
/// authorization caching is disabled" — so 0 is the lowest value it takes.
pub const MIN_AUTHORIZER_RESULT_TTL_SECONDS: i32 = 0;

/// From `Authorizer$authorizerResultTtlInSeconds`'s own documentation: "The maximum
/// value is 3600, or 1 hour."
pub const MAX_AUTHORIZER_RESULT_TTL_SECONDS: i32 = 3600;

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

/// A caller-supplied map as it is STORED and reported: absent when the caller set no
/// entry, so an empty map in the request does not become an empty map in every later
/// read.
///
/// One copy for every operation that stores a caller-sized map (an integration's request
/// parameters and templates, a method response's response parameters and models), so two
/// of them cannot drift on what an empty map means.
pub fn non_empty_map<V>(
    map: Option<::std::collections::HashMap<String, V>>,
) -> Option<::std::collections::HashMap<String, V>> {
    map.filter(|entries| !entries.is_empty())
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

// ---------------------------------------------------------------------------
// Patch operations
//
// Every `Update*` operation of this service is an HTTP PATCH carrying
// `patchOperations`: a list of JSON-Pointer addressed edits that the service applies to
// the stored resource, in request order, before reporting the updated resource. What an
// `op` means, how a `path` is read and how a single-valued member, a string set or a map
// member is edited are shared here, so no two handlers can disagree about it: a handler
// only maps a supported path to the member of its own record that the path names.
// ---------------------------------------------------------------------------

/// The `op` of one [`Patch`]: the values the model's `Op` enum declares
/// (`smithy_query(query="shape", shape="Op")`: add, remove, replace, move, copy, test),
/// which [`crate::ops::types::OP_VALUES`] lists on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchOpKind {
    Add,
    Remove,
    Replace,
    Move,
    Copy,
    Test,
}

impl PatchOpKind {
    /// The kind an `op` string names, or `None` when it names none of them.
    pub fn parse(op: &str) -> Option<Self> {
        match op {
            "add" => Some(Self::Add),
            "remove" => Some(Self::Remove),
            "replace" => Some(Self::Replace),
            "move" => Some(Self::Move),
            "copy" => Some(Self::Copy),
            "test" => Some(Self::Test),
            _ => None,
        }
    }

    /// The wire spelling, for a refusal message that quotes the op back.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Replace => "replace",
            Self::Move => "move",
            Self::Copy => "copy",
            Self::Test => "test",
        }
    }
}

/// Why a patch could not be applied. A handler maps this to the modelled error its own
/// operation declares for that kind of refusal, so the same malformed patch is answered
/// with the same status whichever resource it targeted.
#[derive(Debug, Clone)]
pub enum PatchFailure {
    /// The request is not valid: an unknown op, a path this resource has no updateable
    /// member for, a value that is not of the member's type. `BadRequestException`.
    BadRequest(String),
    /// The patch names a resource that does not exist. `NotFoundException`.
    NotFound(String),
    /// Applying the patch would collide with another stored record.
    /// `ConflictException`.
    Conflict(String),
}

/// One parsed entry of `patchOperations`.
#[derive(Debug, Clone)]
pub struct Patch {
    /// The edit to perform.
    pub op: PatchOpKind,
    /// The `path` exactly as the request spelled it, for a refusal that quotes it back.
    pub path: String,
    /// `path` split into its JSON Pointer reference tokens, each unescaped — so
    /// `/binaryMediaTypes/image~1png` is `["binaryMediaTypes", "image/png"]`.
    pub tokens: Vec<String>,
    /// The op's `value`: "applicable for the add or replace operation"
    /// (`PatchOperation$value`).
    pub value: Option<String>,
    /// The `copy`/`move` source pointer (`PatchOperation$from`).
    pub from: Option<String>,
}

/// Decode one JSON Pointer reference token.
///
/// From `PatchOperation$path`: "Any slash ("/") character appearing in path names must
/// be escaped with "~1"". `~1` is decoded before `~0`, as RFC 6901 requires, so an
/// escaped tilde cannot go on to produce a second escape.
fn unescape_pointer_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

impl Patch {
    /// Parse one wire `PatchOperation`.
    fn parse(operation: &crate::ops::types::PatchOperation) -> Result<Self, PatchFailure> {
        let op = operation.op.as_deref().ok_or_else(|| {
            PatchFailure::BadRequest(
                "each patchOperations entry must carry an op".to_string(),
            )
        })?;
        let op = PatchOpKind::parse(op).ok_or_else(|| {
            PatchFailure::BadRequest(format!(
                "Invalid patch operation specified: {op}; expected one of {}",
                crate::ops::types::OP_VALUES.join(", ")
            ))
        })?;
        let path = operation.path.as_deref().ok_or_else(|| {
            PatchFailure::BadRequest(
                "each patchOperations entry must carry a path".to_string(),
            )
        })?;
        bound_chars("a patchOperations path", path, MAX_STRING_CHARS)
            .map_err(PatchFailure::BadRequest)?;
        // A JSON Pointer to a member of the targeted resource starts at the resource
        // root, so it begins with `/` and names at least one member.
        let tokens: Vec<String> = match path.strip_prefix('/') {
            Some(rest) if !rest.is_empty() => rest.split('/').map(unescape_pointer_token).collect(),
            _ => {
                return Err(PatchFailure::BadRequest(format!(
                    "Invalid patch path: {path}; a path is a JSON Pointer naming a member of the resource, such as /name"
                )))
            }
        };
        Ok(Self {
            op,
            path: path.to_string(),
            tokens,
            value: operation.value.clone(),
            from: operation.from.clone(),
        })
    }

    /// The member the path's first reference token names.
    pub fn member(&self) -> &str {
        self.tokens.first().map_or("", String::as_str)
    }

    /// The op's `value`, refusing an add or replace that carries none: there is nothing
    /// to set the member to.
    pub fn require_value(&self) -> Result<&str, PatchFailure> {
        self.value.as_deref().ok_or_else(|| {
            PatchFailure::BadRequest(format!(
                "a {} of {} requires a value",
                self.op.as_str(),
                self.path
            ))
        })
    }

    /// The op's `value` read as the boolean a boolean member takes.
    pub fn require_bool(&self) -> Result<bool, PatchFailure> {
        let value = self.require_value()?;
        match value.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(PatchFailure::BadRequest(format!(
                "{} takes a boolean value of true or false, got {value:?}",
                self.path
            ))),
        }
    }

    /// The op's `value` read as an integer inside `range`, for a member documented with
    /// bounds.
    pub fn require_i32_in(
        &self,
        range: ::std::ops::RangeInclusive<i32>,
    ) -> Result<i32, PatchFailure> {
        let value = self.require_value()?;
        let parsed: i32 = value.trim().parse().map_err(|_| {
            PatchFailure::BadRequest(format!(
                "{} takes an integer value, got {value:?}",
                self.path
            ))
        })?;
        if !range.contains(&parsed) {
            return Err(PatchFailure::BadRequest(format!(
                "{} must be between {} and {}, inclusive: {parsed}",
                self.path,
                range.start(),
                range.end()
            )));
        }
        Ok(parsed)
    }

    /// This patch as it addresses a member of a NESTED structure: the first `depth`
    /// reference tokens — the ones naming the structure itself — are dropped, so the
    /// structure's own members are addressed exactly the way a top-level member is.
    /// `path` is kept whole, so a refusal still quotes the pointer the request sent.
    pub fn rebased(&self, depth: usize) -> Self {
        Self {
            tokens: self.tokens.iter().skip(depth).cloned().collect(),
            ..self.clone()
        }
    }

    /// Refuse a path that reaches INTO a single-valued member (`/name/first`): such a
    /// member has no sub-member a pointer could name.
    pub fn require_single_valued(&self) -> Result<(), PatchFailure> {
        if self.tokens.len() == 1 {
            return Ok(());
        }
        Err(PatchFailure::BadRequest(format!(
            "Invalid patch path: {}; {} is a single-valued member and has no sub-member",
            self.path,
            self.member()
        )))
    }

    /// The refusal for a path the targeted resource has no updateable member for.
    ///
    /// "Attempts to apply an unsupported operation on a resource will return an error
    /// message" (`PatchOperation$op`), and the message API Gateway returns names the
    /// path, the op and the paths the resource does have — as a recorded refusal shows:
    /// `Invalid patch path  '/id' specified for op 'replace'. Must be one of:
    /// [/description, /enabled, /name, /customerId]`. That is the rule reproduced here,
    /// with each operation's own `supported` list; a caller has to fix the request from
    /// this text alone.
    pub fn unsupported_path(&self, supported: &[&str]) -> PatchFailure {
        PatchFailure::BadRequest(format!(
            "Invalid patch path  '{}' specified for op '{}'. Must be one of: [{}]",
            self.path,
            self.op.as_str(),
            supported.join(", ")
        ))
    }

    /// The refusal for an op the member this path names does not support.
    ///
    /// "Not all valid operations are supported for a given resource. Support of the
    /// operations depends on specific operational contexts. Attempts to apply an
    /// unsupported operation on a resource will return an error message"
    /// (`PatchOperation$op`): the ops these resources support are `add`, `remove` and
    /// `replace` — `move`, `copy` and `test` name nothing on them — and a recorded AWS
    /// refusal of a `move` says exactly `Invalid patch operation specified. Must be
    /// 'add'|'remove'|'replace'`, which is the rule reproduced here.
    pub fn unsupported_op(&self) -> PatchFailure {
        PatchFailure::BadRequest(
            "Invalid patch operation specified. Must be 'add'|'remove'|'replace'".to_string(),
        )
    }

    /// The entry of a collection member this patch names.
    ///
    /// `depth` is how many reference tokens the member itself takes, so the entry is
    /// what the path carries beyond it (`/binaryMediaTypes/image~1png`) and otherwise
    /// the op's own `value` (`op=add, path=/binaryMediaTypes, value=image/png`) — the
    /// two spellings a client can address one entry of a string collection with.
    pub fn collection_entry(&self, depth: usize) -> Result<String, PatchFailure> {
        match self.tokens.get(depth..) {
            Some(rest) if !rest.is_empty() => Ok(rest.join("/")),
            _ => Ok(self.require_value()?.to_string()),
        }
    }

    /// The key of a map member this patch names: the reference token beyond the member,
    /// which is the key, unescaped.
    pub fn map_key(&self, depth: usize) -> Result<String, PatchFailure> {
        match self.tokens.get(depth..) {
            Some(rest) if !rest.is_empty() => Ok(rest.join("/")),
            _ => Err(PatchFailure::BadRequest(format!(
                "Invalid patch path: {}; {} is a map and the path must name one key, as in {}/<key>",
                self.path,
                self.member(),
                self.path.trim_end_matches('/')
            ))),
        }
    }
}

/// Parse a request's `patchOperations` into the edits to apply, in request order.
///
/// Called BEFORE anything is written, so a list this service cannot read leaves the
/// store untouched. The list is caller-sized, so it is bounded like every other
/// caller-sized collection — see [`bound_entries`].
pub fn parse_patches(
    operations: Option<&Vec<crate::ops::types::PatchOperation>>,
) -> Result<Vec<Patch>, PatchFailure> {
    let operations: &[crate::ops::types::PatchOperation] =
        operations.map(Vec::as_slice).unwrap_or_default();
    bound_entries("patchOperations", operations.len(), MAX_COLLECTION_ENTRIES)
        .map_err(PatchFailure::BadRequest)?;
    operations.iter().map(Patch::parse).collect()
}

/// Apply `patch` to a single-valued OPTIONAL string member.
///
/// `add` and `replace` set it to the op's value — `value` is "applicable for the add or
/// replace operation" (`PatchOperation$value`) — and `remove` clears it, which an
/// optional member can be. Any other op is unsupported on such a member.
pub fn patch_optional_string(
    current: &mut Option<String>,
    patch: &Patch,
    max_chars: usize,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_value()?;
            bound_chars(&patch.path, value, max_chars).map_err(PatchFailure::BadRequest)?;
            *current = Some(value.to_string());
        }
        PatchOpKind::Remove => *current = None,
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a single-valued REQUIRED string member: as
/// [`patch_optional_string`], except that `remove` is refused — the member always has a
/// value — and an empty value is refused for the same reason.
pub fn patch_required_string(
    current: &mut String,
    patch: &Patch,
    max_chars: usize,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_value()?;
            if value.is_empty() {
                return Err(PatchFailure::BadRequest(format!(
                    "{} must not be empty",
                    patch.path
                )));
            }
            bound_chars(&patch.path, value, max_chars).map_err(PatchFailure::BadRequest)?;
            *current = value.to_string();
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a single-valued string member whose value set the model declares as
/// an enum: as [`patch_optional_string`], plus the enum's own values, because a patch
/// `value` is a bare string on the wire and so reaches the handler unvalidated.
pub fn patch_enum_string(
    current: &mut Option<String>,
    patch: &Patch,
    values: &[&str],
) -> Result<(), PatchFailure> {
    if matches!(patch.op, PatchOpKind::Add | PatchOpKind::Replace) {
        patch.require_single_valued()?;
        let value = patch.require_value()?;
        if !values.contains(&value) {
            return Err(PatchFailure::BadRequest(format!(
                "{} takes one of {}, got {value:?}",
                patch.path,
                values.join(", ")
            )));
        }
    }
    patch_optional_string(current, patch, MAX_STRING_CHARS)
}

/// Apply `patch` to a boolean member. `remove` is refused: a boolean member declared
/// with a default always has one of its two values.
pub fn patch_bool(current: &mut bool, patch: &Patch) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => *current = patch.require_bool()?,
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a nullable integer member bounded by `range`; `remove` clears it.
pub fn patch_optional_i32(
    current: &mut Option<i32>,
    patch: &Patch,
    range: ::std::ops::RangeInclusive<i32>,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => *current = Some(patch.require_i32_in(range)?),
        PatchOpKind::Remove => *current = None,
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a member that is a set of strings (`binaryMediaTypes`,
/// `authorizationScopes`, an API key's stage associations).
///
/// `add` appends `entry` when the collection does not hold it already, so adding twice
/// is the same as adding once; `remove` drops it. `replace` is unsupported: the path
/// names one entry, not the whole collection. The collection is caller-sized, so it is
/// bounded like every other.
pub fn patch_string_set(
    current: &mut Vec<String>,
    patch: &Patch,
    entry: &str,
    max_entries: usize,
) -> Result<(), PatchFailure> {
    match patch.op {
        PatchOpKind::Add => {
            if !current.iter().any(|held| held == entry) {
                bound_entries(&patch.path, current.len() + 1, max_entries)
                    .map_err(PatchFailure::BadRequest)?;
                current.push(entry.to_string());
            }
        }
        PatchOpKind::Remove => current.retain(|held| held != entry),
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a map member whose values are strings: `add` and `replace` set
/// `key` to the op's value, `remove` drops the key.
pub fn patch_string_map(
    current: &mut BTreeMap<String, String>,
    patch: &Patch,
    key: String,
    max_entries: usize,
) -> Result<(), PatchFailure> {
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_value()?;
            bound_chars(&patch.path, value, MAX_STRING_CHARS).map_err(PatchFailure::BadRequest)?;
            if !current.contains_key(&key) {
                bound_entries(&patch.path, current.len() + 1, max_entries)
                    .map_err(PatchFailure::BadRequest)?;
            }
            current.insert(key, value.to_string());
        }
        PatchOpKind::Remove => {
            current.remove(&key);
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
}

/// Apply `patch` to a map member whose values are booleans; see [`patch_string_map`].
pub fn patch_bool_map(
    current: &mut BTreeMap<String, bool>,
    patch: &Patch,
    key: String,
    max_entries: usize,
) -> Result<(), PatchFailure> {
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_bool()?;
            if !current.contains_key(&key) {
                bound_entries(&patch.path, current.len() + 1, max_entries)
                    .map_err(PatchFailure::BadRequest)?;
            }
            current.insert(key, value);
        }
        PatchOpKind::Remove => {
            current.remove(&key);
        }
        _ => return Err(patch.unsupported_op()),
    }
    Ok(())
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

/// The `NotFoundException` message for a `restApiId` that names no stored API.
///
/// API Gateway does not state a fixed sentence here: it names the identifier it could
/// not resolve, qualified by the account the lookup ran in —
/// `Invalid API identifier specified <accountId>:<restApiId>` — so the message is BUILT
/// FROM THE REQUEST. The account segment is the emulator's one account
/// ([`::emulator_server::emulator_account_id`]), never a literal. Shared so every
/// operation that resolves an API says the same thing about the same failure.
pub fn no_such_rest_api(rest_api_id: &str) -> String {
    format!(
        "Invalid API identifier specified {}:{rest_api_id}",
        ::emulator_server::emulator_account_id()
    )
}

/// The `NotFoundException` message for a `resourceId` that names no stored resource.
pub const NO_SUCH_RESOURCE: &str = "Invalid Resource identifier specified";

/// The `NotFoundException` message for an `httpMethod` with no method on the resource.
pub const NO_SUCH_METHOD: &str = "Invalid Method identifier specified";

/// The `NotFoundException` message for a method that carries no integration.
pub const NO_SUCH_INTEGRATION: &str = "Invalid Integration identifier specified";

/// The `NotFoundException` message for a `statusCode` that names no method response of
/// the method.
pub const NO_SUCH_METHOD_RESPONSE: &str = "Invalid Response status code specified";

/// The `NotFoundException` message for a `stageName` that names no stage of the API.
pub const NO_SUCH_STAGE: &str = "Invalid stage identifier specified";

/// The `NotFoundException` message for a `deploymentId` that names no stored deployment.
pub const NO_SUCH_DEPLOYMENT: &str = "Invalid deployment identifier specified";

/// The `NotFoundException` message for an `apiKey` that names no stored API key.
pub const NO_SUCH_API_KEY: &str = "Invalid API Key identifier specified";

/// The `NotFoundException` message for a `documentationPartId` that names no stored
/// documentation part of the API.
pub const NO_SUCH_DOCUMENTATION_PART: &str = "Invalid Documentation part identifier specified";

/// The `NotFoundException` message for an `authorizerId` that names no stored authorizer
/// of the API.
pub const NO_SUCH_AUTHORIZER: &str = "Invalid Authorizer identifier specified";

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

/// The instant a record is stamped with, at the resolution this surface reports a
/// timestamp in: WHOLE epoch seconds.
///
/// API Gateway's REST-JSON responses carry `Timestamp` members as whole-second epoch
/// numbers, so a stored instant is truncated to the second when it is taken rather than
/// when it is serialized: a client that parses the member as an integer epoch, and
/// anything that compares or hashes the serialized value, then reads the same value the
/// service reports.
pub fn now() -> ::aws_smithy_types::DateTime {
    let seconds = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(seconds).unwrap_or(0))
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
