// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigateway-backplanecontrolservice service.
//!
//! This holds the REST API resource tree the control plane persists — a `RestApi`, the
//! `Resource`s under it (including the implicit root resource every API is created
//! with), the `Method`s put on those resources, the API's `Stage`s and the gateway
//! responses a developer has customized on it — beside the
//! account's `ApiKey`, `UsagePlan` and custom `DomainName` records. Every handler in `src/ops/`
//! computes its response from these records; nothing is synthesised per request.
//!
//! A few leaf values are stored as the modelled shapes from [`crate::ops::types`]
//! (`EndpointConfiguration`, `Method`'s responses and integration) rather than as a
//! second hand-written mirror of the same members: the model already declares them, and
//! two copies of one shape is how two operations end up reading two different records.
//!
//! Two collections of this surface are the SERVICE's own rather than an account's — the
//! gateway responses API Gateway generates and the `SdkType`s it can generate a client
//! SDK for. Neither is created, updated or deleted by any operation of this service, so
//! each is a catalog here ([`default_gateway_response`], [`sdk_type`]) that every
//! handler reading it answers from, instead of a per-account record a caller could never
//! have written.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    ApiStage, CanarySettings, DocumentationPartLocation, EndpointConfiguration,
    GetGatewayResponseOutput, GetIntegrationOutput, GetMethodResponseOutput, GetSdkTypeOutput,
    MethodSetting, MethodSnapshot, MutualTlsAuthentication, QuotaSettings,
    SdkConfigurationProperty, ThrottleSettings,
};

/// Per-service state for com-amazonaws-apigateway-backplanecontrolservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApigatewayBackplanecontrolserviceState {
    /// Every created `RestApi`, keyed by its API id.
    pub rest_apis: BTreeMap<String, RestApiEntry>,
    /// Every created `ApiKey`, keyed by its API key id — the identifier `GetApiKey`
    /// and `DeleteApiKey` take in their `apiKey` path label, NOT the key's value.
    pub api_keys: BTreeMap<String, ApiKeyEntry>,
    /// Every created `UsagePlan`, keyed by its usage plan id — the identifier
    /// `GetUsagePlan` takes in its `usagePlanId` path label.
    pub usage_plans: BTreeMap<String, UsagePlanEntry>,
    /// Every created custom `DomainName`, keyed by [`domain_name_key`]: the domain name
    /// itself, or `<domainName>+<domainNameId>` for a private one, which is addressed by
    /// both members because several private domain names may carry one name.
    pub domain_names: BTreeMap<String, DomainNameEntry>,
    /// The account-level settings `UpdateAccount` writes and `GetAccount` reports.
    /// There is exactly one per account, so it is a field rather than a map.
    pub account: AccountEntry,
    /// Drives [`ComAmazonawsApigatewayBackplanecontrolserviceState::mint_id`]; see there for how an id is derived from it.
    pub id_seed: u64,
    /// Drives [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`]: the creation rank stamped on each
    /// record so a listing can report creation order rather than id order.
    pub sequence_counter: u64,
}

/// The account's own `Account` settings: what `GetAccount` reports and `UpdateAccount`
/// patches.
///
/// The two values a caller never sets — the request limits API Gateway applies to the
/// account and the version of the API keys it issues — are the service's own and are
/// seeded from [`DEFAULT_ACCOUNT_THROTTLE`] / [`API_KEY_VERSION`]; `cloudwatch_role_arn`
/// is absent until a caller sets one, which is what distinguishes an account with no
/// CloudWatch role from one whose role is configured.
#[derive(Debug, Clone)]
pub struct AccountEntry {
    /// The ARN of the IAM role API Gateway uses to write logs to CloudWatch Logs;
    /// `None` on an account that has never had one set.
    pub cloudwatch_role_arn: Option<String>,
    /// The account-wide request limits, in `(burstLimit, rateLimit)` order; see
    /// [`DEFAULT_ACCOUNT_THROTTLE`].
    pub throttle_burst_limit: i32,
    /// See [`DEFAULT_ACCOUNT_THROTTLE`].
    pub throttle_rate_limit: f64,
    /// The features enabled for the account, reported in the `features` list.
    pub features: Vec<String>,
    /// The version of the API keys the account issues; see [`API_KEY_VERSION`].
    pub api_key_version: String,
}

impl Default for AccountEntry {
    fn default() -> Self {
        Self {
            cloudwatch_role_arn: None,
            throttle_burst_limit: DEFAULT_ACCOUNT_THROTTLE.0,
            throttle_rate_limit: DEFAULT_ACCOUNT_THROTTLE.1,
            features: vec![USAGE_PLANS_FEATURE.to_string()],
            api_key_version: API_KEY_VERSION.to_string(),
        }
    }
}

/// The account-wide request limits API Gateway applies, as `(burstLimit, rateLimit)`.
///
/// From API Gateway's published default quota for an account in one Region: "10,000
/// requests per second (RPS) with an additional burst capacity provided by the token
/// bucket algorithm, using a maximum bucket capacity of 5,000 requests" — the pair
/// `GetAccount` reports as `throttleSettings`.
pub const DEFAULT_ACCOUNT_THROTTLE: (i32, f64) = (5_000, 10_000.0);

/// The `features` entry an account with usage plans enabled carries, from
/// `Account$features`'s own documentation: "When usage plans are enabled, the features
/// list will include an entry of `UsagePlans`". It is an account-level flag of the
/// Region's API Gateway, not a claim about which operations of this emulator are
/// implemented, and AWS reports it for every account.
pub const USAGE_PLANS_FEATURE: &str = "UsagePlans";

/// The `apiKeyVersion` of an account: the current version of the API keys API Gateway
/// issues, which every account reports.
pub const API_KEY_VERSION: &str = "4";

/// The `patchOperations` path `UpdateAccount` applies: the account's CloudWatch role.
/// It is the only member of `Account` a caller sets — the others are the service's own
/// (see [`AccountEntry`]) — so it is the only path this operation supports.
pub const ACCOUNT_CLOUDWATCH_ROLE_PATH: &str = "cloudwatchRoleArn";

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
    /// The gateway responses a developer has CUSTOMIZED on this API, keyed by their
    /// `responseType`; written by `PutGatewayResponse` and reported by
    /// `GetGatewayResponse` and `GetGatewayResponses`.
    ///
    /// A type with no entry here is not missing: the API still has a gateway response
    /// for it, the one API Gateway generates itself, which
    /// [`default_gateway_response`] builds. So this map holds only the customizations,
    /// and a stored record is always reported with `defaultResponse: false`.
    pub gateway_responses: BTreeMap<String, GetGatewayResponseOutput>,
    /// Every `DocumentationPart` of this API, keyed by documentation part id: the
    /// records `CreateDocumentationPart` and `ImportDocumentationParts` write and
    /// `GetDocumentationPart` / `GetDocumentationParts` report.
    pub documentation_parts: BTreeMap<String, DocumentationPartEntry>,
    /// Every `DocumentationVersion` of this API, keyed by its version identifier — the
    /// identifier `CreateDocumentationVersion` mints the snapshot under and
    /// `GetDocumentationVersion` takes in its `documentationVersion` path label. A
    /// version identifier is the record's KEY, which is what makes a second snapshot
    /// under one identifier a conflict.
    pub documentation_versions: BTreeMap<String, DocumentationVersionEntry>,
    /// Every `Authorizer` of this API, keyed by authorizer id — the identifier a
    /// `Method`'s `authorizerId` names and `GetAuthorizer` takes in its path label.
    pub authorizers: BTreeMap<String, AuthorizerEntry>,
    /// Every `RequestValidator` of this API, keyed by request validator id — the
    /// identifier a `Method`'s `requestValidatorId` names and `GetRequestValidator`
    /// takes in its path label.
    pub request_validators: BTreeMap<String, RequestValidatorEntry>,
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
    /// When the LAST cache flush `FlushStageCache` started on this stage finishes. A
    /// cluster is reported `FLUSH_IN_PROGRESS` until this instant and `AVAILABLE` from
    /// it on, so an instant already in the past means no flush is outstanding; absent
    /// until the stage is flushed at all. See [`CACHE_CLUSTER_FLUSH_SPAN`].
    pub cache_flush_done_at: Option<::std::time::Instant>,
    /// When `FlushStageAuthorizersCache` LAST dropped this stage's authorizer cache
    /// entries; absent until the stage's authorizer cache is flushed at all.
    ///
    /// This is the stage's authorizer-cache invalidation watermark: an authorizer result
    /// cached before this instant is no longer usable, so a reader of a cached authorizer
    /// result must treat anything older as a miss. Unlike a cache cluster, which a stage
    /// only has when the caller asked for one, EVERY stage has an authorizer cache (each
    /// authorizer caches its result for its own
    /// `authorizerResultTtlInSeconds`), so this watermark is written on every accepted
    /// flush of a stage that exists.
    ///
    /// No operation of this service REPORTS it: API Gateway's `Stage` has no member for
    /// the authorizer cache, so a flush of it leaves every member `GetStage` reports —
    /// `cacheClusterStatus` included — exactly as it was.
    pub authorizers_cache_flushed_at: Option<::std::time::Instant>,
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
    /// than by sleeping or by being read. A cluster a `FlushStageCache` call emptied
    /// reports `FLUSH_IN_PROGRESS` over [`CACHE_CLUSTER_FLUSH_SPAN`] the same way, and is
    /// `AVAILABLE` again once that instant has passed.
    pub fn cache_cluster_status(&self) -> &'static str {
        let now = ::std::time::Instant::now();
        match self.cache_cluster_ready_at {
            None => CACHE_CLUSTER_NOT_AVAILABLE,
            Some(ready_at) if now < ready_at => CACHE_CLUSTER_CREATE_IN_PROGRESS,
            Some(_) => {
                if self.cache_flush_done_at.is_some_and(|done_at| now < done_at) {
                    CACHE_CLUSTER_FLUSH_IN_PROGRESS
                } else {
                    CACHE_CLUSTER_AVAILABLE
                }
            }
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
    /// The API's resource tree as it stood when this deployment was created, keyed by
    /// resource id exactly as [`RestApiEntry::resources`] is.
    ///
    /// A deployment is a SNAPSHOT: "a Deployment resource is a snapshot of your REST API
    /// configuration", and a stage serves the deployment it points at, so this — and not
    /// the live tree — is what the execute-api data plane
    /// ([`crate::service::execute_api`]) matches a request against. An integration put on
    /// a method after this deployment was created is therefore not served until the API
    /// is redeployed, which is what AWS does. `apiSummary` cannot stand in for it: the
    /// modelled `MethodSnapshot` carries only `authorizationType` and `apiKeyRequired`.
    pub resources: BTreeMap<String, ResourceEntry>,
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

/// One stored `DocumentationVersion`: "a snapshot of the documentation of an API"
/// (`DocumentationVersion` in this service's model), under the version identifier the
/// create minted it as.
///
/// The snapshot is the API's documentation parts as they stood at that instant, kept on
/// the record rather than recomputed from the API's current parts: later calls go on
/// creating, updating and deleting parts, and a snapshot that followed them would not be
/// a snapshot.
#[derive(Debug, Clone, Default)]
pub struct DocumentationVersionEntry {
    /// The version identifier, which is also this record's key in
    /// [`RestApiEntry::documentation_versions`].
    pub version: String,
    pub description: Option<String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    /// The documentation parts this snapshot froze, in the creation order they had then;
    /// what `GetExport` serves for this version once it is implemented.
    pub documentation_parts: Vec<DocumentationPartEntry>,
    /// This version's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing reports creation order rather than version-identifier order.
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
    ///
    /// Its `integrationResponses` map is where `PutIntegrationResponse` writes and
    /// `GetIntegrationResponse` reads, keyed by the response's `statusCode`: an
    /// integration response hangs off the INTEGRATION, so the one record lives on the
    /// integration that owns it rather than in a second map beside it.
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

/// One stored `RequestValidator` of an API: the pair of validation flags an API's
/// methods may name in their `requestValidatorId`.
///
/// `name` is optional on the request and so optional here: a validator created without
/// one carries none, and a read of it reports none rather than a name invented per read.
/// The two flags are NOT optional, because `validateRequestBody` and
/// `validateRequestParameters` are modelled with a default of `false`, so a stored
/// validator always has both.
#[derive(Debug, Clone, Default)]
pub struct RequestValidatorEntry {
    pub id: String,
    pub name: Option<String>,
    pub validate_request_body: bool,
    pub validate_request_parameters: bool,
    /// This validator's creation rank, from
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

/// One stored `UsagePlan`: the throttle and quota limits a set of API keys is metered
/// against, and the API stages those limits apply to.
///
/// `throttle`, `quota` and the per-method throttle map inside each `api_stages` entry are
/// kept as the modelled shapes from [`crate::ops::types`] rather than as a second
/// hand-written mirror of their members, for the reason the module docs give. Each is
/// `Option` because a usage plan need not have one: a plan created without a `quota` has
/// no quota at all, which is what distinguishes it from a plan whose quota limit is zero.
///
/// `product_code` is the Amazon Web Services Marketplace product a plan is sold as. No
/// operation of this service sets it — it is not a member of `CreateUsagePlan`'s input —
/// so it stays absent until one does, and a read reports none rather than a code this
/// emulator would have to invent.
#[derive(Debug, Clone, Default)]
pub struct UsagePlanEntry {
    pub id: String,
    /// `CreateUsagePlanRequest$name` is `@required`, so a stored plan always has one.
    pub name: String,
    pub description: Option<String>,
    /// The API stages the plan's limits apply to, each naming a stored stage of a stored
    /// API at the instant the plan was created or last updated.
    pub api_stages: Vec<ApiStage>,
    pub throttle: Option<ThrottleSettings>,
    pub quota: Option<QuotaSettings>,
    /// See the struct docs: absent, because nothing writes it.
    pub product_code: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// This plan's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than id order.
    pub sequence: u64,
}

/// One stored custom `DomainName`: the name a client reaches an API by, the certificate
/// and endpoint configuration it was created with, and the endpoint names API Gateway
/// assigned it.
///
/// `distribution*` and `regional*` are the endpoint the domain name is served by, and a
/// domain name has exactly the one its endpoint type names: the `types` member documents
/// `EDGE` for "an edge-optimized API and its custom domain name" and `REGIONAL` for "a
/// regional API and its custom domain name", so an edge-optimized record carries the
/// CloudFront distribution's names and a regional one the regional endpoint's. A PRIVATE
/// record carries NEITHER: it is reached through the VPC endpoints its endpoint
/// configuration names (`vpcEndpointIds` is "only supported for `PRIVATE` endpoint
/// type"), not through a public alias. Whichever it has is assigned by the service when
/// the record is created and stored, so every later read reports the same values.
///
/// `regionalHostedZoneId` is NOT among them: it is the Route 53 hosted zone of the
/// Region's API Gateway endpoint, a per-Region constant this service's model carries no
/// value for (the member's doc points at the AWS General Reference table instead), so a
/// stored record reports none rather than a zone id this emulator would have to invent.
/// `distributionHostedZoneId`, whose single value the model DOES state, is stored.
#[derive(Debug, Clone, Default)]
pub struct DomainNameEntry {
    pub domain_name: String,
    /// The identifier API Gateway assigns a PRIVATE custom domain name
    /// (`domainNameId` is "supported only for private custom domain names"), minted from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::mint_id`]; absent for an
    /// edge-optimized or regional one.
    pub domain_name_id: Option<String>,
    pub domain_name_arn: String,
    pub certificate_name: Option<String>,
    pub certificate_arn: Option<String>,
    /// When the certificate this domain name uses was uploaded: the instant the record
    /// was created, and only for a record created WITH a certificate.
    pub certificate_upload_date: Option<::aws_smithy_types::DateTime>,
    pub regional_domain_name: Option<String>,
    pub regional_certificate_name: Option<String>,
    pub regional_certificate_arn: Option<String>,
    pub distribution_domain_name: Option<String>,
    /// `Z2FDTNDATAQYW2` for an edge-optimized domain name, absent otherwise; see
    /// [`EDGE_DISTRIBUTION_HOSTED_ZONE_ID`].
    pub distribution_hosted_zone_id: Option<String>,
    /// What the request set, completed by [`defaulted_endpoint_configuration`], so the
    /// stored configuration always names its endpoint types and its IP address type.
    pub endpoint_configuration: Option<EndpointConfiguration>,
    /// When this domain name's endpoint finishes being set up, after which it reports
    /// [`DOMAIN_NAME_AVAILABLE`]; absent for a domain name that was ready when it was
    /// created. See [`DomainNameEntry::status`] and [`DOMAIN_NAME_CREATE_SPAN`].
    pub available_at: Option<::std::time::Instant>,
    pub domain_name_status_message: Option<String>,
    /// What the request set, or [`DEFAULT_SECURITY_POLICY`]: the service picks a value
    /// at create time, so a stored domain name always has one.
    pub security_policy: Option<String>,
    pub endpoint_access_mode: Option<String>,
    pub mutual_tls_authentication: Option<MutualTlsAuthentication>,
    pub ownership_verification_certificate_arn: Option<String>,
    pub management_policy: Option<String>,
    pub policy: Option<String>,
    pub routing_mode: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// This domain name's creation rank, from
    /// [`ComAmazonawsApigatewayBackplanecontrolserviceState::next_sequence`], so a
    /// listing can report creation order rather than key order.
    pub sequence: u64,
}

impl DomainNameEntry {
    /// The `domainNameStatus` this domain name reports now.
    ///
    /// An edge-optimized domain name is served by a CloudFront distribution API Gateway
    /// creates for it, which is not in place when the create call answers, so the record
    /// is reported [`DOMAIN_NAME_PENDING`] until [`DOMAIN_NAME_CREATE_SPAN`] has passed
    /// and [`DOMAIN_NAME_AVAILABLE`] after it; a regional or private domain name is
    /// ready when it is created and has no span to wait out. The transition is one the
    /// CLOCK owes, materialised from the stored due instant rather than by sleeping or
    /// by being read.
    pub fn status(&self) -> &'static str {
        match self.available_at {
            None => DOMAIN_NAME_AVAILABLE,
            Some(available_at) if ::std::time::Instant::now() < available_at => {
                DOMAIN_NAME_PENDING
            }
            Some(_) => DOMAIN_NAME_AVAILABLE,
        }
    }

    /// The key this record is stored under; see [`domain_name_key`].
    pub fn key(&self) -> String {
        domain_name_key(&self.domain_name, self.domain_name_id.as_deref())
    }
}

/// The key a stored [`DomainNameEntry`] is addressed by, and the cursor a
/// `GetDomainNames` page hands back.
///
/// A `domainNameId` is "supported only for private custom domain names", and a private
/// domain name needs it to be identified at all: AWS lets several private records carry
/// one name, so the name alone does not select one. The two members are therefore joined
/// into `<domainName>+<domainNameId>` — the same form this emulator writes into a
/// private domain name's ARN — while an edge-optimized or regional record, which has no
/// id, is keyed by its name alone.
pub fn domain_name_key(domain_name: &str, domain_name_id: Option<&str>) -> String {
    match domain_name_id {
        Some(domain_name_id) if !domain_name_id.is_empty() => {
            format!("{domain_name}+{domain_name_id}")
        }
        _ => domain_name.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Service-side defaults for a created `RestApi` or custom `DomainName`
//
// Several members are optional on input and yet never absent on a created record: the
// service picks a value when the request sets none, stores it, and reports it from
// every read. The constants below are those values, and
// [`defaulted_endpoint_configuration`] is the one place the nested
// `EndpointConfiguration` is completed, so a record can never be stored
// half-defaulted. The endpoint configuration is shared because the model's own
// `EndpointConfiguration` is: it is "the endpoint configuration to indicate the types
// of endpoints an API (RestApi) or its custom domain name (DomainName) has".
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

/// The endpoint type of a regional API and of its custom domain name, from the `types`
/// member's own documentation ("`REGIONAL` for regional API endpoint setup"). A domain
/// name of this type is served by the Region's API Gateway endpoint rather than by a
/// CloudFront distribution, which is what decides which endpoint members it carries.
pub const REGIONAL_ENDPOINT_TYPE: &str = "REGIONAL";

/// The `endpointConfiguration.ipAddressType` a created API carries when the request
/// sets none and the API is not private: `ipv4`, the first of the two documented values
/// ("Use `ipv4` to allow only IPv4 addresses to invoke an API").
pub const IPV4_ADDRESS_TYPE: &str = "ipv4";

/// The `ipAddressType` a private API carries when the request sets none; see
/// [`PRIVATE_ENDPOINT_TYPE`].
pub const DUALSTACK_ADDRESS_TYPE: &str = "dualstack";

/// The `securityPolicy` a created API carries when the request sets none: `TLS_1_0`, the
/// TLS version API Gateway starts an API on (and the first value of the modelled
/// `SecurityPolicy` enum). A created custom domain name is given the same value by the
/// same rule: the member is optional on input and reported from every read.
pub const DEFAULT_SECURITY_POLICY: &str = "TLS_1_0";

/// The `distributionHostedZoneId` of an edge-optimized custom domain name, from the
/// member's own documentation: "The region-agnostic Amazon Route 53 Hosted Zone ID of
/// the edge-optimized endpoint. The valid value is Z2FDTNDATAQYW2 for all the regions."
pub const EDGE_DISTRIBUTION_HOSTED_ZONE_ID: &str = "Z2FDTNDATAQYW2";

/// The `domainNameStatus` of a custom domain name whose endpoint is in place: one of the
/// two values `DomainName$domainNameStatus` documents ("The valid values are AVAILABLE
/// and UPDATING") and a value of the modelled `DomainNameStatus` enum.
pub const DOMAIN_NAME_AVAILABLE: &str = "AVAILABLE";

/// The `domainNameStatus` of a custom domain name whose endpoint is still being set up —
/// a value of the modelled `DomainNameStatus` enum
/// (`smithy_query(query="shape", shape="DomainNameStatus")`).
pub const DOMAIN_NAME_PENDING: &str = "PENDING";

/// The `distributionDomainName` of an edge-optimized custom domain name: the CloudFront
/// distribution API Gateway puts in front of it, which is "the domain name of the Amazon
/// CloudFront distribution associated with this custom domain name" the caller points a
/// DNS record at. `id` is minted from the store's own id bijection and stored on the
/// record, so the name is assigned once and reported unchanged by every later read.
pub fn edge_distribution_domain_name(id: &str) -> String {
    format!("d{id}.cloudfront.net")
}

/// The `regionalDomainName` of a regional custom domain name: the Region's API Gateway
/// endpoint, which the member documents as "returned by API Gateway when you create a
/// regional endpoint" and which a caller points a DNS record at. The host label is
/// minted and stored the way [`edge_distribution_domain_name`]'s is.
pub fn regional_endpoint_domain_name(id: &str, region: &str) -> String {
    format!("d-{id}.execute-api.{region}.amazonaws.com")
}

/// Complete a request's (possibly absent) `endpointConfiguration` with the values the
/// service fills in, so what is stored is what every read of the record must report.
///
/// An absent or empty `types` becomes [`DEFAULT_ENDPOINT_TYPE`]; an absent
/// `ipAddressType` becomes [`DUALSTACK_ADDRESS_TYPE`] for a private endpoint, which
/// supports no other, and [`IPV4_ADDRESS_TYPE`] otherwise. Whatever the request DID set
/// — its endpoint types, its address type, its `vpcEndpointIds` — is kept as given.
///
/// Shared by `CreateRestApi` and `CreateDomainName`: the shape and its defaulting rule
/// are the model's own, declared for "an API (RestApi) or its custom domain name
/// (DomainName)".
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

// ---------------------------------------------------------------------------
// Gateway responses
//
// Every API has a gateway response for EVERY modelled `GatewayResponseType` from the
// moment it is created: until a developer customizes one, it is the response API
// Gateway generates itself, reported with `defaultResponse: true`. Only the
// customizations are stored (`RestApiEntry::gateway_responses`); the generated ones are
// derived from the catalog below, so a read of an uncustomized type and a read of a
// customized one answer from one rule.
// ---------------------------------------------------------------------------

/// The HTTP status code the gateway response API Gateway GENERATES carries, per
/// `GatewayResponseType`.
///
/// Hand-written because the model carries the type's values and nothing else:
/// `smithy_query(query="shape", shape="com.amazonaws.apigateway#GatewayResponseType")`
/// lists the 21 values with no status code attached, and `GatewayResponse$statusCode`
/// documents only the member. Source: the API Gateway Developer Guide's gateway
/// response table ("Setting up gateway responses to customize error responses"), which
/// lists each response type beside the status code of the response API Gateway returns
/// for it. `DEFAULT_4XX` and `DEFAULT_5XX` are the two rows that carry NO status code of
/// their own — each is a fallback for a whole class of responses, which keep their own
/// code — so they are absent here, which is why [`default_gateway_status_code`] answers
/// an `Option`.
const DEFAULT_GATEWAY_STATUS_CODES: &[(&str, &str)] = &[
    ("ACCESS_DENIED", "403"),
    ("API_CONFIGURATION_ERROR", "500"),
    ("AUTHORIZER_CONFIGURATION_ERROR", "500"),
    ("AUTHORIZER_FAILURE", "500"),
    ("BAD_REQUEST_BODY", "400"),
    ("BAD_REQUEST_PARAMETERS", "400"),
    ("EXPIRED_TOKEN", "403"),
    ("INTEGRATION_FAILURE", "504"),
    ("INTEGRATION_TIMEOUT", "504"),
    ("INVALID_API_KEY", "403"),
    ("INVALID_SIGNATURE", "403"),
    ("MISSING_AUTHENTICATION_TOKEN", "403"),
    ("QUOTA_EXCEEDED", "429"),
    ("REQUEST_TOO_LARGE", "413"),
    ("RESOURCE_NOT_FOUND", "404"),
    ("THROTTLED", "429"),
    ("UNAUTHORIZED", "401"),
    ("UNSUPPORTED_MEDIA_TYPE", "415"),
    ("WAF_FILTERED", "403"),
];

/// The status code [`DEFAULT_GATEWAY_STATUS_CODES`] gives `response_type`, or `None` for
/// a type that has none (`DEFAULT_4XX`, `DEFAULT_5XX`) and for a value the catalog does
/// not name.
///
/// Shared by the operations that write and read a gateway response, so the status code a
/// `PutGatewayResponse` that set none stores is the one a read of the uncustomized type
/// would have reported.
pub fn default_gateway_status_code(response_type: &str) -> Option<&'static str> {
    DEFAULT_GATEWAY_STATUS_CODES
        .iter()
        .find(|(kind, _)| *kind == response_type)
        .map(|(_, status)| *status)
}

/// The gateway response an API reports for `response_type` while no customization is
/// stored for it: the one API Gateway generates, so `defaultResponse` is `true`, the
/// status code is the type's own, and neither caller-supplied map is set.
pub fn default_gateway_response(response_type: &str) -> GetGatewayResponseOutput {
    GetGatewayResponseOutput {
        response_type: Some(response_type.to_string()),
        status_code: default_gateway_status_code(response_type).map(str::to_string),
        response_parameters: None,
        response_templates: None,
        default_response: true,
    }
}

// ---------------------------------------------------------------------------
// SDK types
//
// The `SdkType` collection is the SERVICE's published catalog of the languages API
// Gateway can generate a client SDK in for a deployed stage. It is the same collection
// for every account, no operation of this service creates, updates or deletes an entry
// of it, and an `SdkType` id is exactly what `GetSdk`'s `sdkType` label names. So the
// catalog below IS the collection: `GetSdkTypes` pages it and `GetSdkType` resolves one
// id out of it, the same way an uncustomized gateway response is derived from the
// catalog above rather than from a stored record.
// ---------------------------------------------------------------------------

/// One configuration property of an SDK type, as the catalog spells it:
/// `(name, friendlyName, description, required)`. `name` is the key a `GetSdk`
/// `parameters` entry carries for that property.
type SdkPropertyRow = (&'static str, &'static str, &'static str, bool);

/// One SDK type of the catalog:
/// `(id, friendlyName, description, configurationProperties)`.
type SdkTypeRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static [SdkPropertyRow],
);

/// The SDK types API Gateway can generate a client SDK for.
///
/// Hand-written because this service's model declares no shape enumerating them. The ids
/// and their order are `GetSdkRequest$sdkType`'s own documentation
/// (`smithy_query(query="doc", shape="GetSdkRequest", member="sdkType")`: "The language
/// for the generated SDK. Currently `java`, `javascript`, `android`, `objectivec` (for
/// iOS), `swift` (for iOS), and `ruby` are supported"), and each type's REQUIRED
/// configuration properties are `GetSdkRequest$parameters`' ("For `sdkType` of
/// `objectivec` or `swift`, a parameter named `classPrefix` is required. For `sdkType` of
/// `android`, parameters named `groupId`, `artifactId`, `artifactVersion`, and
/// `invokerPackage` are required. For `sdkType` of `java`, parameters named `serviceName`
/// and `javaPackageName` are required."), in the order that sentence lists them. A type
/// that documentation names no parameter for therefore carries NO configuration
/// property, rather than one this emulator invented.
///
/// The `friendlyName` and `description` prose below is NOT documented anywhere this
/// emulator can read: the model states no sentence for either member, so each one is
/// this emulator's own wording for the type or property the model DOES name, and a
/// client that keys off it rather than off the id is reading prose the real service
/// words differently. Nothing but the ids, their order and the configuration property
/// names and `required` flags is claimed to match AWS.
const SDK_TYPES: &[SdkTypeRow] = &[
    (
        "java",
        "Java SDK",
        "Generates a Java client SDK for a deployed stage of a REST API.",
        &[
            (
                "serviceName",
                "Service name",
                "The name of the service the generated Java client calls.",
                true,
            ),
            (
                "javaPackageName",
                "Java package name",
                "The package the generated Java client classes are declared in.",
                true,
            ),
        ],
    ),
    (
        "javascript",
        "JavaScript SDK",
        "Generates a JavaScript client SDK for a deployed stage of a REST API.",
        &[],
    ),
    (
        "android",
        "Android SDK",
        "Generates an Android client SDK for a deployed stage of a REST API.",
        &[
            (
                "groupId",
                "Group ID",
                "The group id of the generated Android SDK's build artifact.",
                true,
            ),
            (
                "artifactId",
                "Artifact ID",
                "The artifact id of the generated Android SDK's build artifact.",
                true,
            ),
            (
                "artifactVersion",
                "Artifact version",
                "The version of the generated Android SDK's build artifact.",
                true,
            ),
            (
                "invokerPackage",
                "Invoker package",
                "The package the generated Android client classes are declared in.",
                true,
            ),
        ],
    ),
    (
        "objectivec",
        "iOS (Objective-C) SDK",
        "Generates an Objective-C client SDK for a deployed stage of a REST API, for iOS.",
        &[(
            "classPrefix",
            "Class prefix",
            "The prefix the generated Objective-C client classes are named with.",
            true,
        )],
    ),
    (
        "swift",
        "iOS (Swift) SDK",
        "Generates a Swift client SDK for a deployed stage of a REST API, for iOS.",
        &[(
            "classPrefix",
            "Class prefix",
            "The prefix the generated Swift client classes are named with.",
            true,
        )],
    ),
    (
        "ruby",
        "Ruby SDK",
        "Generates a Ruby client SDK for a deployed stage of a REST API.",
        &[],
    ),
];

/// Every SDK type's id, in the catalog's order: the stable sequence `GetSdkTypes` pages
/// and a `position` cursor walks, which [`page_of`] cuts a page out of.
pub fn sdk_type_ids() -> Vec<String> {
    SDK_TYPES.iter().map(|(id, ..)| (*id).to_string()).collect()
}

/// The SDK type `id` names, or `None` when the catalog holds no type under that id.
///
/// Shared by `GetSdkType` and `GetSdkTypes`, so one entry of the listed collection and a
/// direct read of the same id are the same record member for member.
pub fn sdk_type(id: &str) -> Option<GetSdkTypeOutput> {
    SDK_TYPES
        .iter()
        .find(|(type_id, ..)| *type_id == id)
        .map(|(type_id, friendly_name, description, properties)| GetSdkTypeOutput {
            id: Some((*type_id).to_string()),
            friendly_name: Some((*friendly_name).to_string()),
            description: Some((*description).to_string()),
            // Always reported, and empty for a type the documentation names no
            // parameter for: the member says which properties `GetSdk` takes, and "none"
            // is an answer a client can read.
            configuration_properties: Some(
                properties
                    .iter()
                    .map(
                        |(name, property_friendly_name, property_description, required)| {
                            SdkConfigurationProperty {
                                name: Some((*name).to_string()),
                                friendly_name: Some((*property_friendly_name).to_string()),
                                description: Some((*property_description).to_string()),
                                required: *required,
                                // The catalog documents no default for any of these
                                // properties, so none is reported.
                                default_value: None,
                            }
                        },
                    )
                    .collect(),
            ),
        })
}

/// The `NotFoundException` message for an `id` that names no SDK type of the catalog.
///
/// API Gateway states no fixed sentence here, so the message is BUILT FROM THE REQUEST:
/// it names the identifier that could not be resolved, as this service's other
/// identifier refusals do (see [`no_such_rest_api`]).
pub fn no_such_sdk_type(id: &str) -> String {
    format!("Invalid SDK type identifier specified: {id}")
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

/// API Gateway's default quota "Usage plans per account per Region: 300".
pub const MAX_USAGE_PLANS: usize = 300;

/// API Gateway's default quota "Custom domain names (regional or edge-optimized) per
/// account per Region: 120".
pub const MAX_DOMAIN_NAMES: usize = 120;

/// API Gateway's default quota "Resources per API: 300".
pub const MAX_RESOURCES_PER_API: usize = 300;

/// API Gateway's default quota "Documentation parts: 2000 per API".
pub const MAX_DOCUMENTATION_PARTS_PER_API: usize = 2_000;

/// Emulator resource guard, NOT an AWS quota: the most documentation versions this
/// emulator keeps for one API. API Gateway publishes a documentation-PARTS quota (see
/// [`MAX_DOCUMENTATION_PARTS_PER_API`]) but none for the snapshots taken of them, so this
/// bound is the emulator's own, the same order as the deployment guard beside it.
pub const MAX_DOCUMENTATION_VERSIONS_PER_API: usize = 300;

/// API Gateway's default quota "Stages per API: 10".
pub const MAX_STAGES_PER_API: usize = 10;

/// API Gateway's default quota "Authorizers per API: 10".
pub const MAX_AUTHORIZERS_PER_API: usize = 10;

/// Emulator resource guard, NOT an AWS quota: the most deployments this emulator keeps
/// for one API. API Gateway publishes no deployment quota.
pub const MAX_DEPLOYMENTS_PER_API: usize = 300;

/// Emulator resource guard, NOT an AWS quota: the most request validators this emulator
/// keeps for one API. API Gateway publishes no request-validator quota, so this bound is
/// the emulator's own, the same order as the deployment guard above.
pub const MAX_REQUEST_VALIDATORS_PER_API: usize = 300;

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

/// The `cacheClusterStatus` of a cache cluster whose entries are being flushed.
pub const CACHE_CLUSTER_FLUSH_IN_PROGRESS: &str = "FLUSH_IN_PROGRESS";

/// How long a stage's cache cluster stays `FLUSH_IN_PROGRESS` after
/// `FlushStageCache` empties it, after which it reports `AVAILABLE` again. API Gateway
/// documents no duration for a flush, so this span is the emulator's own, chosen so a
/// caller reading straight after `FlushStageCache` still observes `FLUSH_IN_PROGRESS`.
pub const CACHE_CLUSTER_FLUSH_SPAN: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

/// How long an edge-optimized custom domain name takes to become
/// [`DOMAIN_NAME_AVAILABLE`]: the CloudFront distribution API Gateway creates for it is
/// not in place when the create call answers. API Gateway documents no duration for it
/// ("it can take up to 40 minutes" is guidance, not a contract), so this span is the
/// emulator's own, chosen short enough not to slow a caller down and long enough that a
/// caller reading straight after `CreateDomainName` still observes
/// [`DOMAIN_NAME_PENDING`].
pub const DOMAIN_NAME_CREATE_SPAN: ::std::time::Duration =
    ::std::time::Duration::from_millis(500);

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

// ---------------------------------------------------------------------------
// Taggable resources, addressed by ARN
//
// `GetTags`, `TagResource` and `UntagResource` name their target by ARN rather than by
// the path labels every other operation of this service takes, so reading an ARN into
// the record it names lives HERE, in one place all three share: the same string must
// resolve to the same record, and be refused the same way, whichever of them took it.
//
// WHICH resources are taggable comes from the model: a resource of this service is
// taggable exactly when the shape declared for it carries a `tags` member
// (`smithy_query(query="shape", shape="RestApi,Stage,ApiKey,UsagePlan,DomainName,\
// ClientCertificate,VpcLink,DomainNameAccessAssociation")` — all eight do), and the
// resource portion of such a resource's ARN is the URI of the operation that reads it
// (`smithy_query(query="operation", operation="GetRestApi,GetStage,GetApiKey,\
// GetUsagePlan,GetDomainName,GetClientCertificate,GetVpcLink")`: `/restapis/{restApiId}`,
// `/restapis/{restApiId}/stages/{stageName}`, `/apikeys/{apiKey}`, and so on).
// ---------------------------------------------------------------------------

/// The vendor field an ARN of one of this service's resources carries: this service's
/// own `aws.api#service$arnNamespace` (`smithy_query(query="service_info")`:
/// `arnNamespace: apigateway`), which is what says an ARN names a resource this API can
/// address at all.
const ARN_SERVICE: &str = "apigateway";

/// The leading resource-portion segment of each taggable resource kind whose records
/// this emulator does not keep: the operations that create a client certificate, a VPC
/// link or a domain name access association are still scaffolds, so nothing can answer to
/// one of these ARNs — while the ARN itself is a well-formed ARN of a taggable API
/// Gateway resource and so is not a malformed request.
///
/// The segments are the first path segment of each kind's read URI; see the section
/// comment above for the `smithy_query` calls they come from.
const UNSTORED_TAGGABLE_KINDS: &[&str] = &[
    "clientcertificates",
    "vpclinks",
    "domainnameaccessassociations",
];

/// The resource one `resourceArn` names: the stored record a tag collection hangs off,
/// as [`parse_tagged_resource`] read it out of the ARN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// `/restapis/{restApiId}` — a [`RestApiEntry`]'s tags.
    RestApi { rest_api_id: String },
    /// `/restapis/{restApiId}/stages/{stageName}` — a [`StageEntry`]'s tags.
    Stage {
        rest_api_id: String,
        stage_name: String,
    },
    /// `/apikeys/{apiKey}` — an [`ApiKeyEntry`]'s tags.
    ApiKey { api_key_id: String },
    /// `/usageplans/{usagePlanId}` — a [`UsagePlanEntry`]'s tags.
    UsagePlan { usage_plan_id: String },
    /// `/domainnames/{domainName}` — a [`DomainNameEntry`]'s tags, where the path
    /// segment is the record's [`domain_name_key`].
    DomainName { domain_name_key: String },
    /// A taggable kind this emulator keeps no records of; see
    /// [`UNSTORED_TAGGABLE_KINDS`]. `kind` is that list's entry and `id` the identifier
    /// the ARN carried, so the refusal can name what it could not resolve.
    Unstored { kind: &'static str, id: String },
}

/// Read `resource_arn` as the ARN of a taggable resource of this service.
///
/// The six ARN fields come from [`::service_common::arn::parse_of`], the ONE parse of a
/// caller-supplied ARN, narrowed to this service's vendor namespace — so what counts as
/// an ARN, and whose resource it names, is what every other service of this emulator
/// says. Only the resource PORTION is read here, because its shape is this service's
/// own.
///
/// `None` for anything that is not the ARN of a taggable resource — a string that is not
/// an ARN, an ARN of another service, or an API Gateway ARN whose resource portion names
/// something this service does not tag (a resource or a deployment of an API, a usage
/// plan key) — which the caller is told is a bad request rather than a missing resource.
///
/// The region and account fields are not matched further: this emulator serves one
/// account, and nothing it stores is keyed by partition or region, so what selects a
/// record is the resource portion.
pub fn parse_tagged_resource(resource_arn: &str) -> Option<TaggedResource> {
    let arn = ::service_common::arn::parse_of(resource_arn, ARN_SERVICE)?;
    let path: Vec<&str> = arn.resource.strip_prefix('/')?.split('/').collect();
    if path.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    match path.as_slice() {
        ["restapis", rest_api_id] => Some(TaggedResource::RestApi {
            rest_api_id: (*rest_api_id).to_string(),
        }),
        ["restapis", rest_api_id, "stages", stage_name] => Some(TaggedResource::Stage {
            rest_api_id: (*rest_api_id).to_string(),
            stage_name: (*stage_name).to_string(),
        }),
        ["apikeys", api_key_id] => Some(TaggedResource::ApiKey {
            api_key_id: (*api_key_id).to_string(),
        }),
        ["usageplans", usage_plan_id] => Some(TaggedResource::UsagePlan {
            usage_plan_id: (*usage_plan_id).to_string(),
        }),
        ["domainnames", domain_name_key] => Some(TaggedResource::DomainName {
            domain_name_key: (*domain_name_key).to_string(),
        }),
        [kind, id] => UNSTORED_TAGGABLE_KINDS
            .iter()
            .copied()
            .find(|known| known == kind)
            .map(|known| TaggedResource::Unstored {
                kind: known,
                id: (*id).to_string(),
            }),
        _ => None,
    }
}

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

/// Refuse a caller-supplied `domainName` that is not a domain name.
///
/// A custom domain name is a DNS name a client resolves and then presents in its TLS
/// handshake, so what this service can serve is bounded by what DNS allows: dot-separated
/// labels of ASCII letters, digits and hyphens that neither begin nor end with a hyphen,
/// at most [`MAX_DOMAIN_LABEL_CHARS`] each and [`MAX_DOMAIN_NAME_CHARS`] in total, with a
/// leading `*` label allowed because AWS serves wildcard custom domain names. The model
/// declares no constraint on the member, so the check lives here and the refusal names
/// what was rejected.
///
/// `Err` carries the message, for the caller to fix the request from; a refused name
/// never reaches the store.
pub fn bound_domain_name(domain_name: &str) -> Result<(), String> {
    bound_chars("domainName", domain_name, MAX_DOMAIN_NAME_CHARS)?;
    let labels: Vec<&str> = domain_name.split('.').collect();
    let well_formed = !domain_name.is_empty()
        && labels.len() > 1
        && labels.iter().enumerate().all(|(index, label)| {
            let wildcard = index == 0 && *label == "*";
            wildcard
                || (!label.is_empty()
                    && label.chars().count() <= MAX_DOMAIN_LABEL_CHARS
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '-'))
        });
    if !well_formed {
        return Err(format!(
            "domainName must be a domain name of dot-separated labels of letters, digits \
             and inner hyphens, got {domain_name:?}"
        ));
    }
    Ok(())
}

/// The longest a domain name can be: 253 characters, the DNS limit on a fully qualified
/// name (RFC 1035's 255-octet wire form, less its length and root bytes).
pub const MAX_DOMAIN_NAME_CHARS: usize = 253;

/// The longest one dot-separated label of a domain name can be: 63 characters (RFC 1035).
pub const MAX_DOMAIN_LABEL_CHARS: usize = 63;

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

/// A stored caller-supplied map as the shared map-patch helpers take it: the modelled
/// shapes carry `HashMap`s that are absent when empty, while a patch is applied to a
/// `BTreeMap` that is present-or-empty, so this is the one place the two spellings meet.
pub fn ordered_map<V>(
    stored: Option<::std::collections::HashMap<String, V>>,
) -> BTreeMap<String, V> {
    stored.unwrap_or_default().into_iter().collect()
}

/// The inverse of [`ordered_map`]: a patched map as it is STORED and reported — absent
/// once its last entry is removed, which is what [`non_empty_map`] means by an empty
/// caller-supplied map.
pub fn stored_map<V>(
    patched: BTreeMap<String, V>,
) -> Option<::std::collections::HashMap<String, V>> {
    non_empty_map(Some(patched.into_iter().collect()))
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

/// The map key a patch names, refused when it is longer than a stored key may be.
///
/// `depth` is how many reference tokens the map member itself takes, so the key is what
/// the path carries beyond it. One copy for every handler that patches a map member, so
/// two of them cannot disagree about how a key is read or how long one may be.
pub fn bounded_map_key(patch: &Patch, depth: usize) -> Result<String, PatchFailure> {
    let key = patch.map_key(depth)?;
    bound_chars(&patch.path, &key, MAX_STRING_CHARS).map_err(PatchFailure::BadRequest)?;
    Ok(key)
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

/// Apply `patch` to an integer member bounded by `range` that always has a value: `add`
/// and `replace` set it, and `remove` is refused, because a member declared with a
/// default is never absent from the record.
pub fn patch_i32(
    current: &mut i32,
    patch: &Patch,
    range: ::std::ops::RangeInclusive<i32>,
) -> Result<(), PatchFailure> {
    patch.require_single_valued()?;
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => *current = patch.require_i32_in(range)?,
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
///
/// `max_chars` bounds one entry's VALUE, which differs by member — a mapping template is
/// far longer than a model name — so each caller passes the bound its own member carries,
/// the same one the create operation for that member enforces.
pub fn patch_string_map(
    current: &mut BTreeMap<String, String>,
    patch: &Patch,
    key: String,
    max_entries: usize,
    max_chars: usize,
) -> Result<(), PatchFailure> {
    match patch.op {
        PatchOpKind::Add | PatchOpKind::Replace => {
            let value = patch.require_value()?;
            bound_chars(&patch.path, value, max_chars).map_err(PatchFailure::BadRequest)?;
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

    /// The keys of every stored custom domain name in creation order, the stable
    /// sequence [`page_of`] cuts a `GetDomainNames` page out of. See
    /// [`domain_name_key`] for what one key is.
    pub fn domain_name_keys_in_creation_order(&self) -> Vec<String> {
        let mut ordered: Vec<&DomainNameEntry> = self.domain_names.values().collect();
        ordered.sort_by_key(|domain| (domain.sequence, domain.key()));
        ordered.into_iter().map(DomainNameEntry::key).collect()
    }

    /// The tag collection stored on the resource a `resourceArn` named, as
    /// [`parse_tagged_resource`] read it.
    ///
    /// One lookup for every tag operation, so what a tag read reports and what a tag
    /// write edits can never be two different records. `Err` carries the
    /// `NotFoundException` message for an ARN that is well formed and that no stored
    /// record answers to, in the wording this service already uses for the same
    /// unresolvable identifier.
    pub fn tags_of(&self, resource: &TaggedResource) -> Result<&BTreeMap<String, String>, String> {
        match resource {
            TaggedResource::RestApi { rest_api_id } => self
                .rest_apis
                .get(rest_api_id)
                .map(|api| &api.tags)
                .ok_or_else(|| no_such_rest_api(rest_api_id)),
            TaggedResource::Stage {
                rest_api_id,
                stage_name,
            } => self
                .rest_apis
                .get(rest_api_id)
                .ok_or_else(|| no_such_rest_api(rest_api_id))?
                .stages
                .get(stage_name)
                .map(|stage| &stage.tags)
                .ok_or_else(|| NO_SUCH_STAGE.to_string()),
            TaggedResource::ApiKey { api_key_id } => self
                .api_keys
                .get(api_key_id)
                .map(|key| &key.tags)
                .ok_or_else(|| NO_SUCH_API_KEY.to_string()),
            TaggedResource::UsagePlan { usage_plan_id } => self
                .usage_plans
                .get(usage_plan_id)
                .map(|plan| &plan.tags)
                .ok_or_else(|| NO_SUCH_USAGE_PLAN.to_string()),
            TaggedResource::DomainName { domain_name_key } => self
                .domain_names
                .get(domain_name_key)
                .map(|domain| &domain.tags)
                .ok_or_else(|| NO_SUCH_DOMAIN_NAME.to_string()),
            TaggedResource::Unstored { kind, id } => Err(no_such_tagged_resource(kind, id)),
        }
    }

    /// The same collection for WRITING, so `TagResource` and `UntagResource` edit the
    /// very record [`Self::tags_of`] reports; see there for the failure.
    pub fn tags_of_mut(
        &mut self,
        resource: &TaggedResource,
    ) -> Result<&mut BTreeMap<String, String>, String> {
        match resource {
            TaggedResource::RestApi { rest_api_id } => self
                .rest_apis
                .get_mut(rest_api_id)
                .map(|api| &mut api.tags)
                .ok_or_else(|| no_such_rest_api(rest_api_id)),
            TaggedResource::Stage {
                rest_api_id,
                stage_name,
            } => self
                .rest_apis
                .get_mut(rest_api_id)
                .ok_or_else(|| no_such_rest_api(rest_api_id))?
                .stages
                .get_mut(stage_name)
                .map(|stage| &mut stage.tags)
                .ok_or_else(|| NO_SUCH_STAGE.to_string()),
            TaggedResource::ApiKey { api_key_id } => self
                .api_keys
                .get_mut(api_key_id)
                .map(|key| &mut key.tags)
                .ok_or_else(|| NO_SUCH_API_KEY.to_string()),
            TaggedResource::UsagePlan { usage_plan_id } => self
                .usage_plans
                .get_mut(usage_plan_id)
                .map(|plan| &mut plan.tags)
                .ok_or_else(|| NO_SUCH_USAGE_PLAN.to_string()),
            TaggedResource::DomainName { domain_name_key } => self
                .domain_names
                .get_mut(domain_name_key)
                .map(|domain| &mut domain.tags)
                .ok_or_else(|| NO_SUCH_DOMAIN_NAME.to_string()),
            TaggedResource::Unstored { kind, id } => Err(no_such_tagged_resource(kind, id)),
        }
    }
}

// ---------------------------------------------------------------------------
// Tagging: the tags this service will STORE on the resource an ARN named.
// Which resource that is comes from `parse_tagged_resource` above, the one place
// this service reads a `resourceArn`; what follows is the one place it reads a tag.
// ---------------------------------------------------------------------------

/// The prefix AWS reserves for its OWN tag keys. From the `tags` member's own
/// documentation: a tag key "must not start with aws:".
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Refuse one caller-supplied tag this service will not store.
///
/// Every bound comes from the `tags` member's own documentation — "The tag key can be
/// up to 128 characters and must not start with aws:. The tag value can be up to 256
/// characters." — so the one check serves every operation that takes tags, whether the
/// tags ride on a create (`CreateRestApi`, `CreateStage`, `CreateApiKey`) or on
/// `TagResource`. The member's documented "valid character set is [a-zA-Z+-=._:/]" is
/// NOT enforced: AWS accepts a key and a value outside it (the recorded units
/// `apigateway.tag-resource-unicode-key-value-round-trip` and
/// `apigateway.tag-value-unicode-round-trip` round-trip non-ASCII tags), so refusing
/// them would be stricter than the service.
pub fn bound_tag(key: &str, value: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("a tag key may not be empty".to_string());
    }
    bound_chars("a tag key", key, MAX_TAG_KEY_CHARS)?;
    bound_chars("a tag value", value, MAX_TAG_VALUE_CHARS)?;
    // The reserved namespace is AWS's own, and a prefix is a prefix whatever its case.
    if key
        .to_ascii_lowercase()
        .starts_with(RESERVED_TAG_KEY_PREFIX)
    {
        return Err(format!(
            "a tag key must not start with the reserved prefix {RESERVED_TAG_KEY_PREFIX}, \
             got {key}"
        ));
    }
    Ok(())
}

/// Refuse a `ThrottleSettings` whose limits are not a request rate API Gateway can
/// meter.
///
/// `burstLimit` is a count of requests ("The API target request burst rate limit") and
/// `rateLimit` a requests-per-second rate ("The API target request rate limit") —
/// `ThrottleSettings`' own members — so neither can be negative, and a rate must be a
/// finite number. Real AWS answers 400 `BadRequestException` for a negative or otherwise
/// invalid throttle rate.
///
/// One check for every place this shape rides on a request — a usage plan's own
/// `throttle`, the per-method throttle map on each of its `apiStages` entries, and any
/// later operation that takes the shape — so one value is refused the same way wherever
/// it appears. `member` is the path the message names, so the caller can see which one to
/// fix.
pub fn bound_throttle_settings(member: &str, throttle: &ThrottleSettings) -> Result<(), String> {
    if throttle.burst_limit < 0 {
        return Err(format!(
            "{member}.burstLimit must not be negative, got {}",
            throttle.burst_limit
        ));
    }
    if !throttle.rate_limit.is_finite() || throttle.rate_limit < 0.0 {
        return Err(format!(
            "{member}.rateLimit must be a non-negative number, got {}",
            throttle.rate_limit
        ));
    }
    Ok(())
}

/// Refuse a `QuotaSettings` whose counts are not a quota API Gateway can meter.
///
/// `limit` is "The target maximum number of requests that can be made in a given time
/// period" and `offset` "The number of requests subtracted from the given limit in the
/// initial time period" (`QuotaSettings`' own members): both count requests, so neither
/// can be negative, and real AWS answers 400 `BadRequestException` for a negative quota
/// limit.
///
/// `period` is NOT checked here: it targets the modelled `QuotaPeriodType` enum, so a
/// value outside that set is refused at the boundary before any handler runs.
pub fn bound_quota_settings(member: &str, quota: &QuotaSettings) -> Result<(), String> {
    for (count_member, count) in [("limit", quota.limit), ("offset", quota.offset)] {
        if count < 0 {
            return Err(format!(
                "{member}.{count_member} must not be negative, got {count}"
            ));
        }
    }
    Ok(())
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

/// The `NotFoundException` message for a `statusCode` that names no response of the
/// method at that code — the method response `GetMethodResponse` resolves, or the
/// integration response `GetIntegrationResponse` resolves on the method's integration.
/// Both are keyed by the same `StatusCode`, so both say the same thing about it.
pub const NO_SUCH_METHOD_RESPONSE: &str = "Invalid Response status code specified";

/// The `NotFoundException` message for a `stageName` that names no stage of the API.
pub const NO_SUCH_STAGE: &str = "Invalid stage identifier specified";

/// The `NotFoundException` message for a `deploymentId` that names no stored deployment.
pub const NO_SUCH_DEPLOYMENT: &str = "Invalid deployment identifier specified";

/// The `NotFoundException` message for an `apiKey` that names no stored API key.
pub const NO_SUCH_API_KEY: &str = "Invalid API Key identifier specified";

/// The `NotFoundException` message for a `usagePlanId` that names no stored usage plan,
/// in the shape of the sibling identifier messages above. Shared by every operation that
/// resolves a usage plan, so all of them say the same thing about the same failure.
pub const NO_SUCH_USAGE_PLAN: &str = "Invalid Usage Plan ID specified";

/// The `NotFoundException` message for a `domainName` (with its `domainNameId`, for a
/// private one) that names no stored custom domain name, in the shape of the sibling
/// identifier messages above. Shared by every operation that resolves a domain name, so
/// all of them say the same thing about the same failure.
pub const NO_SUCH_DOMAIN_NAME: &str = "Invalid domain name identifier specified";

/// The `NotFoundException` message for a well-formed ARN of a taggable resource kind
/// this emulator keeps no records of (see [`UNSTORED_TAGGABLE_KINDS`]): nothing answers
/// to it, which is a missing resource and not a malformed request.
///
/// AWS states no fixed sentence here either, so the message is BUILT FROM THE REQUEST —
/// it names the resource portion the lookup could not resolve, qualified by the account
/// the lookup ran in, the way [`no_such_rest_api`] does.
pub fn no_such_tagged_resource(kind: &str, id: &str) -> String {
    format!(
        "Invalid resource identifier specified {}:/{kind}/{id}",
        ::emulator_server::emulator_account_id()
    )
}

/// The `BadRequestException` message for a `resourceArn` that is not the ARN of a
/// taggable resource of this service — the refusal [`parse_tagged_resource`]'s `None`
/// earns. BUILT FROM THE REQUEST: it names the value that could not be read as such an
/// ARN, so the caller can see which argument to fix.
pub fn invalid_tagged_resource_arn(resource_arn: &str) -> String {
    format!("Invalid ARN specified in the request {resource_arn}")
}

/// The `NotFoundException` message for a `documentationPartId` that names no stored
/// documentation part of the API.
pub const NO_SUCH_DOCUMENTATION_PART: &str = "Invalid Documentation part identifier specified";

/// The `NotFoundException` message for a documentation version identifier that names no
/// snapshot of the API, in the shape of the sibling identifier messages above.
pub const NO_SUCH_DOCUMENTATION_VERSION: &str = "Invalid Documentation version specified";

/// The `NotFoundException` message for an `authorizerId` that names no stored authorizer
/// of the API.
pub const NO_SUCH_AUTHORIZER: &str = "Invalid Authorizer identifier specified";

/// The `NotFoundException` message for a `requestValidatorId` that names no stored
/// request validator of the API, in the shape of the sibling identifier messages above.
pub const NO_SUCH_REQUEST_VALIDATOR: &str = "Invalid Request Validator identifier specified";

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

/// The Region a request is scoped to: the Region of its SigV4 credential scope, or
/// [`DEFAULT_REGION`] for a request that carries none.
///
/// A custom domain name's ARN and its regional endpoint name both name the Region the
/// record was created in, which is the Region the caller addressed.
pub fn request_region(parts: &http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region a request that carries no SigV4 credential scope is treated as addressing:
/// `us-east-1`, AWS's own default Region for a client configured with none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of a stored resource of this service, whose resource portion is the URI of
/// the operation that reads it (see the "Taggable resources" section above).
///
/// API Gateway's own ARNs carry an EMPTY account field —
/// `arn:aws:apigateway:{region}::/domainnames/{domainName}` — so this is not
/// [`::emulator_server::emulator_arn`], which always writes the account in. The
/// partition and the vendor are the ones [`parse_tagged_resource`] reads back, so an ARN
/// this service REPORTS is one it can resolve again.
pub fn service_arn(region: &str, resource_path: &str) -> String {
    format!("arn:aws:{ARN_SERVICE}:{region}::{resource_path}")
}

/// The ARN of the stored custom domain name addressed by `key` (a [`domain_name_key`]).
pub fn domain_name_arn(region: &str, key: &str) -> String {
    service_arn(region, &format!("/domainnames/{key}"))
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
