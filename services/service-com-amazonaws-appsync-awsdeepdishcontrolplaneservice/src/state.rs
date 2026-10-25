// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appsync-awsdeepdishcontrolplaneservice service.
//!
//! Holds the API lifecycle: every `Api` and every `GraphqlApi` the service created, each with
//! the API keys and data sources created under it, and every source-API association
//! between two of them. A handler reaches it through [`service_state`] and answers from what is
//! stored here — nothing is recomputed from the request.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AdditionalAuthenticationProvider, Api, ApiCache, ApiKey, AppSyncRuntime, AuthMode, DataSource,
    DynamodbDataSourceConfig, ElasticsearchDataSourceConfig, EnhancedMetricsConfig, EventConfig,
    EventBridgeDataSourceConfig, FunctionConfiguration, GraphqlApi, HttpDataSourceConfig,
    LambdaAuthorizerConfig, LambdaDataSourceConfig, LogConfig, OpenIDConnectConfig,
    OpenSearchServiceDataSourceConfig, RelationalDatabaseDataSourceConfig, SourceApiAssociation,
    SourceApiAssociationConfig, SyncConfig, UserPoolConfig,
};

/// One stored `GraphqlApi`, plus the API keys created under it.
///
/// Every member of the modelled `GraphqlApi` shape that the create call can set is
/// kept, so `GetGraphqlApi` and `ListGraphqlApis` project exactly what was created
/// rather than a subset.
#[derive(Debug, Clone)]
pub struct GraphqlApiRecord {
    /// Creation order, used to keep `ListGraphqlApis` (and its `nextToken` cursor)
    /// stable across calls: the minted `apiId` is opaque, so it cannot carry order.
    pub seq: u64,
    pub api_id: String,
    pub name: String,
    pub arn: String,
    pub authentication_type: String,
    pub api_type: String,
    pub visibility: String,
    pub introspection_config: String,
    /// The account that owns the API — this emulator's account id.
    pub owner: String,
    pub owner_contact: Option<String>,
    pub xray_enabled: bool,
    pub query_depth_limit: i32,
    pub resolver_count_limit: i32,
    pub merged_api_execution_role_arn: Option<String>,
    pub log_config: Option<LogConfig>,
    pub user_pool_config: Option<UserPoolConfig>,
    pub open_id_connect_config: Option<OpenIDConnectConfig>,
    pub lambda_authorizer_config: Option<LambdaAuthorizerConfig>,
    pub additional_authentication_providers: Vec<AdditionalAuthenticationProvider>,
    pub enhanced_metrics_config: Option<EnhancedMetricsConfig>,
    /// The API's DNS records — one hostname per endpoint, keyed as AppSync keys
    /// them (`GRAPHQL`, `REALTIME`). `uris` is derived from these, so the two
    /// members can never disagree about where the API is served.
    pub dns: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    /// The API's environmental variables, the whole list the last write left
    /// behind: a write REPLACES this map rather than merging into it.
    pub environment_variables: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// The API keys created under this API, keyed by key id.
    pub api_keys: BTreeMap<String, ApiKeyRecord>,
    /// The API's cache, or `None` while it has none: an API holds at most one
    /// `ApiCache` (`CreateApiCache` "Creates a cache for the GraphQL API", and
    /// `GetApiCache` resolves it from the `apiId` alone).
    pub api_cache: Option<ApiCacheRecord>,
    /// The data sources created under this API, keyed by their caller-supplied
    /// name — the name is what every data-source operation addresses one by, so it
    /// identifies the data source within its API and no second key is needed.
    /// Bounded: `CreateDataSource` refuses a create once the API holds the AppSync
    /// per-API quota of data sources, so this map cannot grow without limit.
    pub data_sources: BTreeMap<String, DataSourceRecord>,
    /// The functions created under this API, keyed by the opaque `functionId` the
    /// create minted for each — a function name "does not have to be unique"
    /// (`CreateFunctionRequest$name`), so the id is what identifies one within its
    /// API and the name cannot be the key.
    /// Bounded: `CreateFunction` refuses a create once the API holds the AppSync
    /// per-API quota of functions, so this map cannot grow without limit.
    pub functions: BTreeMap<String, FunctionRecord>,
}

impl GraphqlApiRecord {
    /// This record as the modelled `GraphqlApi` shape.
    ///
    /// The single projection used by create, get and list, so all three answer the
    /// same body for the same stored API.
    pub fn to_graphql_api(&self) -> GraphqlApi {
        GraphqlApi {
            name: Some(self.name.clone()),
            api_id: Some(self.api_id.clone()),
            authentication_type: Some(self.authentication_type.clone()),
            log_config: self.log_config.clone(),
            user_pool_config: self.user_pool_config.clone(),
            open_id_connect_config: self.open_id_connect_config.clone(),
            arn: Some(self.arn.clone()),
            uris: Some(uris_from_dns(&self.dns)),
            // The tag map is reported whether or not it holds tags, so a caller
            // reads the same member on every API. `TagMap`'s modelled minimum
            // length of 1 is not the rule for the RESPONSE: the recorded creates
            // this projection answers carry `tags` on an API created without any
            // (replaying them diverged at `$.graphqlApi.tags` while the member was
            // omitted, and stopped diverging once it was reported).
            tags: Some(map_out(&self.tags)),
            additional_authentication_providers: (!self
                .additional_authentication_providers
                .is_empty())
            .then(|| self.additional_authentication_providers.clone()),
            xray_enabled: self.xray_enabled,
            // Associating a WAF web ACL and a custom domain are their own
            // operations, still unimplemented, so nothing can have set either.
            waf_web_acl_arn: None,
            lambda_authorizer_config: self.lambda_authorizer_config.clone(),
            // "The DNS records for the API" — the hostnames the endpoints in
            // `uris` are served on.
            dns: Some(map_out(&self.dns)),
            visibility: Some(self.visibility.clone()),
            api_type: Some(self.api_type.clone()),
            merged_api_execution_role_arn: self.merged_api_execution_role_arn.clone(),
            owner: Some(self.owner.clone()),
            owner_contact: self.owner_contact.clone(),
            introspection_config: Some(self.introspection_config.clone()),
            query_depth_limit: self.query_depth_limit,
            resolver_count_limit: self.resolver_count_limit,
            enhanced_metrics_config: self.enhanced_metrics_config.clone(),
        }
    }
}

/// One stored `ApiCache` of a `GraphqlApiRecord`.
///
/// Every member of the modelled `ApiCache` shape the create call can set is kept, so
/// the create and `GetApiCache` project exactly what was created rather than a subset.
#[derive(Debug, Clone)]
pub struct ApiCacheRecord {
    pub ttl: i64,
    pub api_caching_behavior: String,
    pub r#type: String,
    pub transit_encryption_enabled: bool,
    pub at_rest_encryption_enabled: bool,
    pub health_metrics_config: Option<String>,
    /// The `ApiCacheStatus` the cache instance is in.
    pub status: String,
    /// The instant the cache the clock is provisioning (or re-provisioning) becomes
    /// available, or `None` once nothing is owed. Stored rather than slept on: see
    /// [`CACHE_SPAN`].
    pub available_at: Option<SystemTime>,
    /// The instant the cache's entries were last discarded, or `None` while nothing
    /// has discarded them. Not a member of the modelled `ApiCache`, so no operation
    /// projects it: it is read only by whatever serves an entry.
    ///
    /// A cache entry is a response the GraphQL endpoint served, so the entries are
    /// written by the data plane rather than by any control-plane call: what a flush
    /// leaves behind is this boundary, after which no entry cached before it is
    /// served. [`ApiCacheRecord::flush`] moves it; nothing else does.
    pub last_flushed_at: Option<SystemTime>,
}

impl ApiCacheRecord {
    /// Materialise the transition to [`CACHE_AVAILABLE`] the CLOCK already made due
    /// at `now`.
    ///
    /// Owed by the clock alone: the due instant was recorded by the call that started
    /// the provisioning, so the same transition is applied whether or not anybody read
    /// the cache in between.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.available_at else {
            return;
        };
        if now < due {
            return;
        }
        self.available_at = None;
        self.status = CACHE_AVAILABLE.to_string();
    }

    /// Discard every entry the cache holds, as of `now`.
    ///
    /// The instance itself is untouched — its settings and the status it is in are
    /// properties of the instance, not of the entries it holds — so the only thing a
    /// flush leaves behind is the new entry boundary ([`Self::last_flushed_at`]).
    pub fn flush(&mut self, now: SystemTime) {
        self.last_flushed_at = Some(now);
    }

    /// This record as the modelled `ApiCache` shape — the single projection the
    /// create and the read both answer with.
    pub fn to_api_cache(&self) -> ApiCache {
        ApiCache {
            ttl: self.ttl,
            api_caching_behavior: Some(self.api_caching_behavior.clone()),
            transit_encryption_enabled: self.transit_encryption_enabled,
            at_rest_encryption_enabled: self.at_rest_encryption_enabled,
            r#type: Some(self.r#type.clone()),
            status: Some(self.status.clone()),
            health_metrics_config: self.health_metrics_config.clone(),
        }
    }
}

/// One stored API key of a `GraphqlApiRecord`.
#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    /// Creation order within the parent API, for a stable `ListApiKeys` page order.
    pub seq: u64,
    pub id: String,
    pub description: Option<String>,
    /// Epoch seconds, floored to the hour — the model documents both `expires` and
    /// `deletes` as "seconds since the epoch, rounded down to the nearest hour".
    pub expires: i64,
    pub deletes: i64,
}

impl ApiKeyRecord {
    /// A key record with `expires` as the call that minted it asked for, set through
    /// [`ApiKeyRecord::set_expiration`] so a fresh key's two time members are built by
    /// the same rule an update applies to them.
    pub fn new(seq: u64, id: String, description: Option<String>, expires: i64) -> Self {
        let mut record = Self {
            seq,
            id,
            description,
            expires: 0,
            deletes: 0,
        };
        record.set_expiration(expires);
        record
    }

    /// This record as the modelled `ApiKey` shape.
    pub fn to_api_key(&self) -> ApiKey {
        ApiKey {
            id: Some(self.id.clone()),
            description: self.description.clone(),
            expires: self.expires,
            deletes: self.deletes,
        }
    }

    /// Set the key's expiration, in epoch seconds, to the instant the call that sets
    /// it asked for.
    ///
    /// Both `expires` and `deletes` are documented as "seconds since the epoch,
    /// rounded down to the nearest hour", so the instant is floored here rather than
    /// by each caller; the deletion time moves with the expiration, since the model
    /// states no span between an API key expiring and being deleted.
    pub fn set_expiration(&mut self, expires: i64) {
        self.expires = floor_to_hour(expires);
        self.deletes = self.expires;
    }
}

/// One hour in seconds: the granularity `ApiKey$expires` and `ApiKey$deletes` are
/// documented with — both are "seconds since the epoch, rounded down to the nearest
/// hour".
pub const SECONDS_PER_HOUR: i64 = 3_600;

/// One day in seconds: the unit the API-key validity window is stated in.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// The shortest validity an API key accepts, measured from the call that sets it.
///
/// Source: `ApiKeyValidityOutOfBoundsException` — "The API key expiration must be set
/// to a value between 1 and 365 days from creation (for CreateApiKey) or from update
/// (for UpdateApiKey)".
pub const API_KEY_MIN_VALIDITY_SECONDS: i64 = SECONDS_PER_DAY;

/// The longest validity an API key accepts ([`API_KEY_MIN_VALIDITY_SECONDS`] states
/// the source).
pub const API_KEY_MAX_VALIDITY_SECONDS: i64 = 365 * SECONDS_PER_DAY;

/// Whether `validity` seconds is inside the documented API-key validity window
/// ([`API_KEY_MIN_VALIDITY_SECONDS`]..=[`API_KEY_MAX_VALIDITY_SECONDS`]).
///
/// The window is measured from the call that sets the expiration, so a create and an
/// update judge the same span against the same bounds.
pub fn is_api_key_validity(validity: i64) -> bool {
    (API_KEY_MIN_VALIDITY_SECONDS..=API_KEY_MAX_VALIDITY_SECONDS).contains(&validity)
}

/// AppSync's fixed text for a validity outside that window, carried by
/// `ApiKeyValidityOutOfBoundsException` (HTTP 400): the bound the validity crossed,
/// stated in days, so the caller can correct the value from the message alone. The
/// rejected value is not echoed — the bound is what the caller has to stay within.
pub fn api_key_validity_out_of_bounds_message(validity: i64) -> String {
    if validity < API_KEY_MIN_VALIDITY_SECONDS {
        format!(
            "API key must be valid for a minimum of {} days.",
            API_KEY_MIN_VALIDITY_SECONDS / SECONDS_PER_DAY
        )
    } else {
        format!(
            "API key can be valid for a maximum of {} days.",
            API_KEY_MAX_VALIDITY_SECONDS / SECONDS_PER_DAY
        )
    }
}

/// `seconds` rounded DOWN to the nearest hour, the form both `ApiKey$expires` and
/// `ApiKey$deletes` are documented to take.
pub fn floor_to_hour(seconds: i64) -> i64 {
    seconds.saturating_sub(seconds.rem_euclid(SECONDS_PER_HOUR))
}

/// Now, as whole seconds since the Unix epoch — the form an API key's expiration takes.
pub fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The text `NotFoundException` (HTTP 404) carries for an API key its parent API does
/// not hold. The requirement the refusal answers is the status and the error shape; no
/// recording pins AppSync's own wording, so this states the absent resource.
pub const API_KEY_NOT_FOUND_MESSAGE: &str = "API key not found.";

/// One stored data source of a `GraphqlApiRecord`.
///
/// Every member of the modelled `DataSource` shape that a create or update call can
/// set is kept, including each per-type configuration, so a reader projects exactly
/// what was written rather than a subset. `description` is kept as an `Option`, so a
/// data source that has no description reports none rather than an empty one — see
/// [`data_source_description`] for which requests leave it with none.
#[derive(Debug, Clone)]
pub struct DataSourceRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    /// The `DataSourceType` this data source is of.
    pub r#type: String,
    pub service_role_arn: Option<String>,
    pub dynamodb_config: Option<DynamodbDataSourceConfig>,
    pub lambda_config: Option<LambdaDataSourceConfig>,
    pub elasticsearch_config: Option<ElasticsearchDataSourceConfig>,
    pub open_search_service_config: Option<OpenSearchServiceDataSourceConfig>,
    pub http_config: Option<HttpDataSourceConfig>,
    pub relational_database_config: Option<RelationalDatabaseDataSourceConfig>,
    pub event_bridge_config: Option<EventBridgeDataSourceConfig>,
    /// The `DataSourceLevelMetricsConfig` the request asked for, or `None` when it
    /// asked for none: the model documents no default for it.
    pub metrics_config: Option<String>,
}

impl DataSourceRecord {
    /// The configuration member this data source's `type` is set up through and which
    /// it does not carry, or `None` when the type carries it (or needs none at all).
    ///
    /// Source: the per-type configuration members of `CreateDataSourceRequest` /
    /// `UpdateDataSourceRequest` and the `DataSourceType` values they configure —
    /// `dynamodbConfig` ("Amazon DynamoDB settings"), `lambdaConfig` ("Lambda
    /// settings"), `elasticsearchConfig` / `openSearchServiceConfig` ("Amazon
    /// OpenSearch Service settings"), `httpConfig` ("HTTP endpoint settings"),
    /// `relationalDatabaseConfig` ("Relational database settings") and
    /// `eventBridgeConfig` ("Amazon EventBridge settings"). `NONE` and
    /// `AMAZON_BEDROCK_RUNTIME` are configured by no member of either request, so a
    /// data source of either type is complete without one.
    pub fn missing_config(&self) -> Option<&'static str> {
        match self.r#type.as_str() {
            "AMAZON_DYNAMODB" if self.dynamodb_config.is_none() => Some("dynamodbConfig"),
            "AWS_LAMBDA" if self.lambda_config.is_none() => Some("lambdaConfig"),
            "AMAZON_ELASTICSEARCH" if self.elasticsearch_config.is_none() => {
                Some("elasticsearchConfig")
            }
            "AMAZON_OPENSEARCH_SERVICE" if self.open_search_service_config.is_none() => {
                Some("openSearchServiceConfig")
            }
            "HTTP" if self.http_config.is_none() => Some("httpConfig"),
            "RELATIONAL_DATABASE" if self.relational_database_config.is_none() => {
                Some("relationalDatabaseConfig")
            }
            "AMAZON_EVENTBRIDGE" if self.event_bridge_config.is_none() => Some("eventBridgeConfig"),
            _ => None,
        }
    }

    /// This record as the modelled `DataSource` shape.
    ///
    /// The single projection every operation that reports a data source answers with,
    /// so the create and the reads cannot disagree about one data source.
    pub fn to_data_source(&self) -> DataSource {
        DataSource {
            data_source_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            r#type: Some(self.r#type.clone()),
            service_role_arn: self.service_role_arn.clone(),
            dynamodb_config: self.dynamodb_config.clone(),
            lambda_config: self.lambda_config.clone(),
            elasticsearch_config: self.elasticsearch_config.clone(),
            open_search_service_config: self.open_search_service_config.clone(),
            http_config: self.http_config.clone(),
            relational_database_config: self.relational_database_config.clone(),
            event_bridge_config: self.event_bridge_config.clone(),
            metrics_config: self.metrics_config.clone(),
        }
    }
}

/// The description a data source keeps for the `description` a request carried.
///
/// An empty description describes nothing, so no description is kept for it: a data
/// source whose request left `description` empty reports the member absent, exactly
/// as one whose request omitted it. (The recorded creates this projection answers
/// report no description for an explicitly empty one.)
pub fn data_source_description(description: Option<String>) -> Option<String> {
    description.filter(|description| !description.is_empty())
}

/// The resource portion of a data source's ARN: a data source is a child of its API,
/// so it is named under the API's own `apis/{apiId}` path, as `GetDataSource`'s URI
/// (`/v1/apis/{apiId}/datasources/{name}`) addresses it.
pub fn data_source_resource(api_id: &str, name: &str) -> String {
    format!("apis/{api_id}/datasources/{name}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a data source the named API
/// does not hold: the absent data source, which is what the caller has to act on.
///
/// The requirement the refusal answers is the status and the error shape; the wording
/// is the one a recorded `GetDataSource` on a name no create left behind answers with,
/// which is a fixed template over the requested name rather than anything of the
/// request's own values.
pub fn data_source_not_found_message(name: &str) -> String {
    format!("Data source {name} not found")
}

/// The text a `BadRequestException` (HTTP 400) carries for a create whose name is
/// already taken within the API: the name, which is the member the caller has to
/// change.
pub fn data_source_exists_message(name: &str) -> String {
    format!("Data source with name {name} already exists.")
}

/// The text a `BadRequestException` (HTTP 400) carries for a data source whose type
/// is configured through a member the request did not carry
/// ([`DataSourceRecord::missing_config`]): the backend that configuration describes,
/// so the caller can fix the request from the message alone.
///
/// The backend each member names is the subject of that member's own doc on
/// `CreateDataSourceRequest` / `UpdateDataSourceRequest` — `lambdaConfig` "the new
/// Lambda configuration", `dynamodbConfig` "the new Amazon DynamoDB configuration",
/// `elasticsearchConfig` and `openSearchServiceConfig` "the new OpenSearch
/// configuration", `httpConfig` "the new HTTP endpoint configuration",
/// `relationalDatabaseConfig` "the new relational database configuration" and
/// `eventBridgeConfig` "the new Amazon EventBridge settings". A configuration member
/// added to the request later falls back to its own name.
pub fn missing_data_source_config_message(member: &str) -> String {
    let subject = match member {
        "lambdaConfig" => "Lambda",
        "dynamodbConfig" => "Amazon DynamoDB",
        "elasticsearchConfig" | "openSearchServiceConfig" => "OpenSearch",
        "httpConfig" => "HTTP endpoint",
        "relationalDatabaseConfig" => "relational database",
        "eventBridgeConfig" => "Amazon EventBridge",
        other => other,
    };
    format!("Missing {subject} data source config")
}

/// One stored function of a `GraphqlApiRecord`: "A function is a reusable entity. You
/// can use multiple functions to compose the resolver logic."
///
/// Every member of the modelled `FunctionConfiguration` shape a create call can set is
/// kept, so a reader projects exactly what was written rather than a subset.
#[derive(Debug, Clone)]
pub struct FunctionRecord {
    /// Creation order, used to keep a listing (and its `nextToken` cursor) stable
    /// across calls: the minted `functionId` is opaque, so it cannot carry order.
    pub seq: u64,
    pub function_id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub data_source_name: String,
    pub request_mapping_template: Option<String>,
    pub response_mapping_template: Option<String>,
    pub function_version: Option<String>,
    pub sync_config: Option<SyncConfig>,
    pub max_batch_size: i32,
    pub runtime: Option<AppSyncRuntime>,
    pub code: Option<String>,
}

impl FunctionRecord {
    /// This record as the modelled `FunctionConfiguration` shape.
    ///
    /// The single projection every operation that reports a function answers with, so
    /// the create and the reads cannot disagree about one function.
    pub fn to_function_configuration(&self) -> FunctionConfiguration {
        FunctionConfiguration {
            function_id: Some(self.function_id.clone()),
            function_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            data_source_name: Some(self.data_source_name.clone()),
            request_mapping_template: self.request_mapping_template.clone(),
            response_mapping_template: self.response_mapping_template.clone(),
            function_version: self.function_version.clone(),
            sync_config: self.sync_config.clone(),
            max_batch_size: self.max_batch_size,
            runtime: self.runtime.clone(),
            code: self.code.clone(),
        }
    }
}

/// The resource portion of a function's ARN: a function is a child of its API, so it
/// is named under the API's own `apis/{apiId}` path, as `GetFunction`'s URI
/// (`/v1/apis/{apiId}/functions/{functionId}`) addresses it.
pub fn function_resource(api_id: &str, function_id: &str) -> String {
    format!("apis/{api_id}/functions/{function_id}")
}

/// The text a `NotFoundException` (HTTP 404) carries for a function the named API does
/// not hold: the absent function's id, which is what the caller has to act on.
///
/// The requirement the refusal answers is the status and the error shape; the wording
/// is a fixed template over the requested id, as the data-source not-found text
/// ([`data_source_not_found_message`]) is over the requested name, rather than
/// anything of the request's own values.
pub fn function_not_found_message(function_id: &str) -> String {
    format!("Function {function_id} not found")
}

/// The refusal text for a function definition AppSync could not run, or `None` when
/// the request describes one it could.
///
/// Three documented requirements, each over the members that carry the definition
/// rather than over one request's values:
///
/// * `code` "When code is used, the runtime is required";
/// * `runtime` "if a runtime is specified, code must also be specified";
/// * `functionVersion` "when using VTL and mapping templates, the functionVersion is
///   required" — a function that carries no `code` is defined in VTL, so it is the
///   one that needs the template version.
///
/// Shared by every operation that takes this member set (`CreateFunction` and
/// `UpdateFunction`), so the two cannot disagree about which definitions are runnable.
pub fn function_definition_refusal(
    code: Option<&str>,
    runtime: Option<&AppSyncRuntime>,
    function_version: Option<&str>,
) -> Option<String> {
    match (code, runtime) {
        (Some(_), None) => Some("Invalid runtime given. A runtime is required when code is specified.".to_string()),
        (None, Some(_)) => Some("Invalid code given. Code is required when a runtime is specified.".to_string()),
        (Some(_), Some(_)) => None,
        (None, None) if function_version.is_none_or(str::is_empty) => Some(
            "Invalid functionVersion given. A functionVersion is required when using mapping \
             templates."
                .to_string(),
        ),
        (None, None) => None,
    }
}

/// One stored `Api`: an AppSync API created with an Event API configuration
/// ("Describes an AppSync API. You can use `Api` for an AppSync API with your
/// preferred configuration, such as an Event API").
///
/// An `Api` is a resource of its own, kept apart from [`GraphqlApiRecord`]: the two
/// are addressed by different operations under different URI prefixes, so an id that
/// names one never names the other.
#[derive(Debug, Clone)]
pub struct EventApiRecord {
    /// Creation order, used to keep `ListApis` (and its `nextToken` cursor) stable
    /// across calls: the minted `apiId` is opaque, so it cannot carry order.
    pub seq: u64,
    pub api_id: String,
    pub name: String,
    pub api_arn: String,
    pub owner_contact: Option<String>,
    pub xray_enabled: bool,
    /// The web ACL associated with the API, or `None` while it has none.
    pub waf_web_acl_arn: Option<String>,
    /// The API's Event API configuration — its authorization providers and the
    /// authorization modes connecting, publishing and subscribing accept.
    pub event_config: EventConfig,
    /// The API's DNS records — one hostname per endpoint, keyed as AppSync keys
    /// them (`HTTP`, `REALTIME`): `Api$dns` "will include an HTTP and a real-time
    /// endpoint".
    pub dns: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub created: SystemTime,
}

impl EventApiRecord {
    /// This record as the modelled `Api` shape.
    ///
    /// The single projection used by create, get and list, so all three answer the
    /// same body for the same stored API.
    pub fn to_api(&self) -> Api {
        Api {
            api_id: Some(self.api_id.clone()),
            name: Some(self.name.clone()),
            owner_contact: self.owner_contact.clone(),
            // Reported whether or not the API holds tags, the way
            // [`GraphqlApiRecord::to_graphql_api`] reports them, so a caller reads
            // the same member on every API of either kind.
            tags: Some(map_out(&self.tags)),
            dns: Some(map_out(&self.dns)),
            api_arn: Some(self.api_arn.clone()),
            created: Some(::aws_smithy_types::DateTime::from(self.created)),
            xray_enabled: self.xray_enabled,
            waf_web_acl_arn: self.waf_web_acl_arn.clone(),
            event_config: Some(self.event_config.clone()),
        }
    }
}

/// The authorization types an Event API configuration sets up, i.e. the types its
/// authorization modes may name.
///
/// A provider identifies ONE way of authenticating the API, so the set holds at most
/// one entry per value of the modelled `AuthenticationType` enum.
pub fn configured_auth_types(event_config: &EventConfig) -> std::collections::BTreeSet<String> {
    event_config
        .auth_providers
        .iter()
        .map(|provider| provider.auth_type.clone())
        .collect()
}

/// The first mode of `modes` whose `authType` is not among `configured`, or `None`
/// when every one of them is.
pub fn unconfigured_auth_mode<'a>(
    modes: &'a [AuthMode],
    configured: &std::collections::BTreeSet<String>,
) -> Option<&'a str> {
    modes
        .iter()
        .find(|mode| !configured.contains(&mode.auth_type))
        .map(|mode| mode.auth_type.as_str())
}

/// The text a `BadRequestException` (HTTP 400) carries for an authorization mode that
/// names a type the API's authorization providers do not set up: the member that
/// carried it and the type, so the caller can fix the request from the message alone.
pub fn unconfigured_auth_mode_message(member: &str, auth_type: &str) -> String {
    format!(
        "{member} specifies authorization mode {auth_type}, which is not configured as an \
         authorization provider for this API."
    )
}

/// Why the Event API configuration a request carries is not well formed, as the text a
/// `BadRequestException` (HTTP 400) states it with, or `None` when it is well formed.
///
/// `EventConfig` is carried by the create and by the update alike, so the rules are
/// stated here once:
///
/// * an authorization type that is set up through a member of its own
///   ([`missing_auth_config`]) is not configured without that member — checked per
///   provider, which the refusal names by its 1-based position, the only way a caller
///   can tell two of them apart;
/// * an authorization type identifies ONE provider, so the API cannot name it twice;
/// * an OIDC provider is identified by its issuer, an absolute URL;
/// * `connectionAuthModes`, `defaultPublishAuthModes` and `defaultSubscribeAuthModes`
///   are each "a list of VALID authorization modes", i.e. modes the API's own
///   authorization providers set up.
pub fn event_config_refusal(event_config: &EventConfig) -> Option<String> {
    let mut configured: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (position, provider) in event_config.auth_providers.iter().enumerate() {
        if let Some(config) = missing_auth_config(
            Some(&provider.auth_type),
            provider.open_id_connect_config.is_some(),
            provider.cognito_config.is_some(),
            provider.lambda_authorizer_config.is_some(),
        ) {
            return Some(format!(
                "{config} is missing for authorization provider {}",
                position + 1
            ));
        }
        if provider
            .open_id_connect_config
            .as_ref()
            .is_some_and(|config| !is_absolute_url(&config.issuer))
        {
            return Some("Issuer is not a valid URL".to_string());
        }
        if !configured.insert(provider.auth_type.clone()) {
            return Some(format!(
                "Authentication type {} for authorization provider {} already specified on the \
                 API. It can only be specified once.",
                provider.auth_type,
                position + 1
            ));
        }
    }

    // Source: the three `EventConfig` members, each "a list of valid authorization
    // modes" for connecting to, publishing on and subscribing to the Event API.
    let mode_lists = [
        ("connectionAuthModes", &event_config.connection_auth_modes),
        (
            "defaultPublishAuthModes",
            &event_config.default_publish_auth_modes,
        ),
        (
            "defaultSubscribeAuthModes",
            &event_config.default_subscribe_auth_modes,
        ),
    ];
    for (member, modes) in mode_lists {
        if let Some(auth_type) = unconfigured_auth_mode(modes, &configured) {
            return Some(unconfigured_auth_mode_message(member, auth_type));
        }
    }

    None
}

/// One stored association between a Merged API and one of its source APIs.
///
/// Both sides are kept by `apiId` AND by ARN, the two forms the modelled
/// `SourceApiAssociation` reports and the two forms an `…Identifier` member accepts,
/// so a reader never has to rebuild either from the request.
#[derive(Debug, Clone)]
pub struct SourceApiAssociationRecord {
    /// Creation order, so a listing over associations has a stable page order the
    /// opaque `associationId` cannot carry.
    pub seq: u64,
    pub association_id: String,
    pub association_arn: String,
    pub source_api_id: String,
    pub source_api_arn: String,
    pub merged_api_id: String,
    pub merged_api_arn: String,
    pub description: Option<String>,
    /// The `MergeType` the association merges under ([`DEFAULT_MERGE_TYPE`] when the
    /// request named none).
    pub merge_type: String,
    /// The `SourceApiAssociationStatus` the association is in.
    pub status: String,
    /// "The detailed message related to the current state of the source API
    /// association" — the text that goes with [`Self::status`].
    pub status_detail: String,
    /// When the association last merged the source API's schema into the Merged API,
    /// or `None` while it never has.
    pub last_successful_merge_date: Option<SystemTime>,
    /// The instant the merge this association scheduled comes due, or `None` once
    /// there is no merge left owed. Stored rather than slept on: see [`MERGE_SPAN`].
    pub merge_due_at: Option<SystemTime>,
    /// The instant the deletion this association scheduled comes due, or `None` while
    /// no deletion was asked for. Stored rather than slept on: see [`DELETION_SPAN`].
    pub deletion_due_at: Option<SystemTime>,
}

impl SourceApiAssociationRecord {
    /// Whether a merge the clock owes this association has not been materialised yet.
    pub fn merge_pending(&self) -> bool {
        self.merge_due_at.is_some()
    }

    /// Whether the deletion of this association has been scheduled and the clock has
    /// not made it due yet.
    pub fn deletion_pending(&self) -> bool {
        self.deletion_due_at.is_some()
    }

    /// Whether a modification of this association is still in progress, i.e. whether
    /// the clock still owes it a merge or a deletion.
    pub fn modification_pending(&self) -> bool {
        self.merge_pending() || self.deletion_pending()
    }

    /// Schedule this association's deletion at `now`.
    ///
    /// The deletion is SCHEDULED rather than performed, so the association is left in
    /// the non-terminal [`DELETION_SCHEDULED`] status with the due instant the clock
    /// settles, and the merge it was still owed is dropped: an association being
    /// deleted has no schema left to merge.
    pub fn schedule_deletion(&mut self, now: SystemTime) {
        self.merge_due_at = None;
        self.deletion_due_at = Some(now + DELETION_SPAN);
        self.status = DELETION_SCHEDULED.to_string();
        self.status_detail = DELETION_SCHEDULED_DETAIL.to_string();
    }

    /// Whether the deletion this association scheduled has come due at `now`, i.e.
    /// whether the association is gone.
    pub fn deletion_due(&self, now: SystemTime) -> bool {
        self.deletion_due_at.is_some_and(|due| now >= due)
    }

    /// Materialise the merge the CLOCK already made due at `now`.
    ///
    /// Owed by the clock alone: the due instant was recorded by the call that
    /// scheduled the merge, so the same transition is applied whether or not anybody
    /// read the association in between, and the merge date is the due instant itself
    /// rather than the moment of the read — so every later read reports the same one.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.merge_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.merge_due_at = None;
        self.status = MERGE_SUCCESS.to_string();
        self.status_detail = MERGE_SUCCESS_DETAIL.to_string();
        self.last_successful_merge_date = Some(due);
    }

    /// Initiate a merge of the source API's schema into the Merged API, and report the
    /// `SourceApiAssociationStatus` the association is left in, or the refusal the call
    /// meets.
    ///
    /// Scheduling is all this does: the merge itself comes due one [`MERGE_SPAN`] later
    /// and [`Self::settle`] materialises it, exactly as the create that first scheduled
    /// one does. A merge the clock already owes IS the merge such a call initiates, so
    /// its due instant is kept rather than pushed out — repeated calls can neither
    /// starve the transition nor leave two merges owed for one association.
    ///
    /// An association whose DELETION the clock still owes is being destroyed and has no
    /// schema left to merge, and that deletion is a modification already in progress, so
    /// the call is refused rather than overwriting the status the deletion left behind.
    pub fn start_merge(&mut self, now: SystemTime) -> Result<String, AssociationRefusal> {
        if self.deletion_pending() {
            return Err(AssociationRefusal::ConcurrentModification(
                CONCURRENT_MODIFICATION_MESSAGE.to_string(),
            ));
        }
        if self.merge_due_at.is_none() {
            self.merge_due_at = Some(now + MERGE_SPAN);
        }
        self.status = MERGE_SCHEDULED.to_string();
        self.status_detail = MERGE_SCHEDULED_DETAIL.to_string();
        Ok(self.status.clone())
    }

    /// This record as the modelled `SourceApiAssociation` shape.
    ///
    /// The single projection every operation that reports an association answers
    /// with, so the create and the reads cannot disagree about one association.
    pub fn to_source_api_association(&self) -> SourceApiAssociation {
        SourceApiAssociation {
            association_id: Some(self.association_id.clone()),
            association_arn: Some(self.association_arn.clone()),
            source_api_id: Some(self.source_api_id.clone()),
            source_api_arn: Some(self.source_api_arn.clone()),
            merged_api_id: Some(self.merged_api_id.clone()),
            merged_api_arn: Some(self.merged_api_arn.clone()),
            description: self.description.clone(),
            source_api_association_config: Some(SourceApiAssociationConfig {
                merge_type: Some(self.merge_type.clone()),
            }),
            source_api_association_status: Some(self.status.clone()),
            source_api_association_status_detail: Some(self.status_detail.clone()),
            last_successful_merge_date: self
                .last_successful_merge_date
                .map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// What a request to associate a source API with a Merged API asks for, in the form
/// both association creates share.
///
/// AppSync creates one and the same link from either side — `AssociateMergedGraphqlApi`
/// addresses the source API and names the Merged API in the body,
/// `AssociateSourceGraphqlApi` the other way round — so the two differ only in which
/// member the path carries and reach [`create_source_api_association`] with the same
/// request.
#[derive(Debug, Clone)]
pub struct SourceApiAssociationRequest {
    /// The Merged API side, in either accepted form ([`is_api_identifier`]).
    pub merged_api_identifier: String,
    /// The source API side, in either accepted form ([`is_api_identifier`]).
    pub source_api_identifier: String,
    pub description: Option<String>,
    /// The `mergeType` the request's `sourceApiAssociationConfig` named, or `None` when
    /// it named none ([`DEFAULT_MERGE_TYPE`] is then stored).
    pub merge_type: Option<String>,
    /// The region the caller addressed, which the minted association ARN carries.
    pub region: String,
}

/// Why a source-API association could not be created, as the HTTP answer every
/// operation that creates one gives it.
///
/// Stated independently of the operation so both association creates refuse the same
/// request the same way; each handler lifts a variant into its own modelled error.
#[derive(Debug, Clone)]
pub enum AssociationRefusal {
    /// The request names something that cannot be associated: `BadRequestException`
    /// (HTTP 400), carrying this text.
    BadRequest(String),
    /// An identifier that names no API this account holds: `NotFoundException`
    /// (HTTP 404), carrying this text.
    NotFound(String),
    /// A modification of the pair's association is still in progress:
    /// `ConcurrentModificationException` (HTTP 409), carrying this text.
    ConcurrentModification(String),
}

/// Why an `(mergedApiIdentifier, associationId)` pair resolves to no one association,
/// as the HTTP answer every operation that addresses an association that way gives it.
///
/// Stated independently of the operation, so the read and the merge refuse the same
/// pair the same way; each handler lifts a variant into its own modelled error. Only
/// the two refusals the LOOKUP itself can meet are here — what an operation then does
/// with the association it found is its own business.
#[derive(Debug, Clone)]
pub enum AssociationLookupRefusal {
    /// The request names an identifier that cannot name an API at all:
    /// `BadRequestException` (HTTP 400), carrying this text.
    BadRequest(String),
    /// The Merged API or the association under it is not one this account holds:
    /// `NotFoundException` (HTTP 404), carrying this text.
    NotFound(String),
}

/// A lookup refusal is one of the refusals an association operation can give, so an
/// operation that both resolves a pair and then acts on it answers both kinds the same
/// way.
impl From<AssociationLookupRefusal> for AssociationRefusal {
    fn from(refusal: AssociationLookupRefusal) -> Self {
        match refusal {
            AssociationLookupRefusal::BadRequest(message) => Self::BadRequest(message),
            AssociationLookupRefusal::NotFound(message) => Self::NotFound(message),
        }
    }
}

/// Create the association `request` asks for and return it as the modelled shape, or
/// the refusal it meets.
///
/// The whole rule both association creates follow, in one place:
///
/// * an identifier that is neither an API id nor an ARN names no API at all, so the
///   request is malformed rather than pointed at a missing resource;
/// * both sides are resolved from the store in either identifier form, so the stored
///   association records each API's own id and ARN rather than whatever the request
///   named it by, and an identifier that resolves to nothing is a not-found;
/// * an association links a standard GraphQL API (the source) to a Merged API, so an
///   API on the wrong side of it is a bad request;
/// * a source API is linked to a Merged API at most once, so a pair that is already
///   associated leaves the call nothing to create: 409 while the existing
///   association's merge is still owed, 400 once it has been performed;
/// * the association's `associationId` is drawn from the state's own counter and its
///   ARN sits under the Merged API, and creating it SCHEDULES the merge — so it is
///   stored in the non-terminal [`MERGE_SCHEDULED`] status with the due instant the
///   clock settles ([`MERGE_SPAN`]).
///
/// The answer is projected from the STORED record, so it is the body
/// `GetSourceApiAssociation` reports for the same association later.
pub fn create_source_api_association(
    state: &mut ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState,
    request: SourceApiAssociationRequest,
    now: SystemTime,
) -> Result<SourceApiAssociation, AssociationRefusal> {
    // Checked on BOTH sides, since either can carry either form.
    for identifier in [
        &request.merged_api_identifier,
        &request.source_api_identifier,
    ] {
        if !is_api_identifier(identifier) {
            return Err(AssociationRefusal::BadRequest(invalid_api_id_message(
                identifier,
            )));
        }
    }

    // Materialise every merge the clock already owes first, so the decisions below are
    // taken against the association state of THIS instant rather than the one the last
    // caller left behind.
    state.settle_associations(now);

    let Some(merged) = api_by_identifier(&state.graphql_apis, &request.merged_api_identifier)
    else {
        return Err(AssociationRefusal::NotFound(
            API_NOT_FOUND_MESSAGE.to_string(),
        ));
    };
    let merged_api_id = merged.api_id.clone();
    let merged_api_arn = merged.arn.clone();
    let merged_api_type = merged.api_type.clone();

    let Some(source) = api_by_identifier(&state.graphql_apis, &request.source_api_identifier)
    else {
        return Err(AssociationRefusal::NotFound(
            API_NOT_FOUND_MESSAGE.to_string(),
        ));
    };
    let source_api_id = source.api_id.clone();
    let source_api_arn = source.arn.clone();
    let source_api_type = source.api_type.clone();

    // "A source API is a GraphQL API that is linked to a merged API"
    // (`SourceApiAssociation`). An API on the wrong side of that sentence exists and is
    // named well, so the request is a bad one rather than a not-found.
    if merged_api_type != MERGED_API_TYPE {
        return Err(AssociationRefusal::BadRequest(format!(
            "API {merged_api_id} is not a Merged API; only an API of type {MERGED_API_TYPE} can \
             be associated with source APIs"
        )));
    }
    if source_api_type != GRAPHQL_API_TYPE {
        return Err(AssociationRefusal::BadRequest(format!(
            "API {source_api_id} is not a source API; only an API of type {GRAPHQL_API_TYPE} can \
             be associated with a Merged API"
        )));
    }

    if let Some(existing) = state.association_for_pair(&merged_api_id, &source_api_id) {
        if existing.modification_pending() {
            return Err(AssociationRefusal::ConcurrentModification(
                CONCURRENT_MODIFICATION_MESSAGE.to_string(),
            ));
        }
        return Err(AssociationRefusal::BadRequest(format!(
            "Source API {source_api_id} is already associated with Merged API {merged_api_id}"
        )));
    }

    // The id is drawn from the state's own counter, never from the request, so
    // associating two different pairs can never collide on it.
    let mut association_id = state.mint_id();
    while state.source_api_associations.contains_key(&association_id) {
        association_id = state.mint_id();
    }
    let seq = state.next_seq();

    let record = SourceApiAssociationRecord {
        seq,
        association_arn: ::engine::emulator_arn(
            ARN_SERVICE,
            &request.region,
            &format!("apis/{merged_api_id}/sourceApiAssociations/{association_id}"),
        ),
        association_id: association_id.clone(),
        source_api_id,
        source_api_arn,
        merged_api_id,
        merged_api_arn,
        description: request.description,
        merge_type: request
            .merge_type
            .unwrap_or_else(|| DEFAULT_MERGE_TYPE.to_string()),
        // Creating the association SCHEDULES the merge; the clock owes its outcome, so
        // nothing here performs it or waits for it.
        status: MERGE_SCHEDULED.to_string(),
        status_detail: MERGE_SCHEDULED_DETAIL.to_string(),
        last_successful_merge_date: None,
        merge_due_at: Some(now + MERGE_SPAN),
        deletion_due_at: None,
    };
    let association = record.to_source_api_association();
    state
        .source_api_associations
        .insert(association_id, record);

    Ok(association)
}

/// Which side of an association the request's path addressed.
///
/// One association links two APIs, and AppSync lets it be addressed from either side —
/// `DisassociateSourceGraphqlApi` by the Merged API, `DisassociateMergedGraphqlApi` by
/// the source API — so this is the only way the two requests differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationSide {
    /// The Merged API side.
    MergedApi,
    /// The source API side.
    SourceApi,
}

impl AssociationSide {
    /// The `apiId` `association` holds on this side.
    pub fn api_id(self, association: &SourceApiAssociationRecord) -> &str {
        match self {
            Self::MergedApi => &association.merged_api_id,
            Self::SourceApi => &association.source_api_id,
        }
    }
}

/// What a request to delete a source-API association asks for, in the form both
/// disassociates share.
#[derive(Debug, Clone)]
pub struct SourceApiDisassociationRequest {
    /// Which side of the association the path addressed.
    pub side: AssociationSide,
    /// That side's API, in either accepted form ([`is_api_identifier`]).
    pub api_identifier: String,
    /// The `associationId` AppSync generated for the association.
    pub association_id: String,
}

/// Delete the association `request` names and return the `SourceApiAssociationStatus`
/// it is left in, or the refusal the request meets.
///
/// The whole rule a disassociate follows, in one place:
///
/// * an identifier that is neither an API id nor an ARN names no API at all, so the
///   request is malformed rather than pointed at a missing resource;
/// * the addressed API is resolved from the store in either identifier form, and one
///   that resolves to nothing is a not-found;
/// * an `associationId` names an association OF the API addressed, so an association
///   under another API on that side is not found under this one;
/// * deleting the association SCHEDULES the deletion, so the call leaves it in the
///   non-terminal [`DELETION_SCHEDULED`] status with the due instant the clock settles
///   ([`DELETION_SPAN`]), and reports that status; the association stops existing once
///   that instant has passed, whether or not anybody calls again.
pub fn delete_source_api_association(
    state: &mut ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState,
    request: SourceApiDisassociationRequest,
    now: SystemTime,
) -> Result<String, AssociationRefusal> {
    if !is_api_identifier(&request.api_identifier) {
        return Err(AssociationRefusal::BadRequest(invalid_api_id_message(
            &request.api_identifier,
        )));
    }

    // Materialise everything the clock already owes first, so this call decides against
    // the association state of THIS instant: an association whose deletion came due is
    // gone before it can be deleted again.
    state.settle_associations(now);

    let Some(api) = api_by_identifier(&state.graphql_apis, &request.api_identifier) else {
        return Err(AssociationRefusal::NotFound(
            API_NOT_FOUND_MESSAGE.to_string(),
        ));
    };
    let api_id = api.api_id.clone();

    let side = request.side;
    let association = match state
        .source_api_associations
        .get_mut(&request.association_id)
    {
        Some(association) if api_id == side.api_id(association) => association,
        _ => {
            return Err(AssociationRefusal::NotFound(
                ASSOCIATION_NOT_FOUND_MESSAGE.to_string(),
            ))
        }
    };

    // Scheduling is idempotent: a second call reaching an association whose deletion is
    // already owed has nothing left to schedule, so it neither moves the due instant nor
    // reports a different status.
    if !association.deletion_pending() {
        association.schedule_deletion(now);
    }

    Ok(association.status.clone())
}

/// Per-service state for com-amazonaws-appsync-awsdeepdishcontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// Every created GraphQL API, keyed by its minted `apiId`.
    pub graphql_apis: BTreeMap<String, GraphqlApiRecord>,
    /// Every created `Api` (an API with an Event API configuration), keyed by its
    /// minted `apiId`. A separate resource from [`Self::graphql_apis`]: the two are
    /// created and addressed by different operations.
    pub event_apis: BTreeMap<String, EventApiRecord>,
    /// Every source-API association, keyed by its minted `associationId`.
    pub source_api_associations: BTreeMap<String, SourceApiAssociationRecord>,
    /// Advanced once per minted id, so no id is ever a function of the request.
    id_seed: u64,
    /// Advanced once per created resource, giving each record its creation order.
    seq_counter: u64,
}

impl ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// Apply every merge the clock owes at `now`, so an association reports the same
    /// state to every caller whether or not another call touched it first.
    pub fn settle_associations(&mut self, now: SystemTime) {
        // A deletion the clock has made due leaves no association behind, so it is
        // applied before the merges: a record that is gone has no status to settle.
        self.source_api_associations
            .retain(|_, association| !association.deletion_due(now));
        for association in self.source_api_associations.values_mut() {
            association.settle(now);
        }
    }

    /// Apply every cache transition the clock owes at `now`, so a cache reports the
    /// same status to every caller whether or not another call touched it first.
    pub fn settle_api_caches(&mut self, now: SystemTime) {
        for api in self.graphql_apis.values_mut() {
            if let Some(cache) = api.api_cache.as_mut() {
                cache.settle(now);
            }
        }
    }

    /// The association between the Merged API `merged_api_id` and the source API
    /// `source_api_id`, or `None` when the two are not associated.
    ///
    /// A source API is linked to a Merged API at most once, so this is the pair's
    /// whole association state.
    pub fn association_for_pair(
        &self,
        merged_api_id: &str,
        source_api_id: &str,
    ) -> Option<&SourceApiAssociationRecord> {
        self.source_api_associations.values().find(|association| {
            association.merged_api_id == merged_api_id && association.source_api_id == source_api_id
        })
    }

    /// The association `association_id` names OF the Merged API `merged_api_identifier`
    /// addresses, or the refusal the pair meets.
    ///
    /// The rule every operation that addresses one association by that pair follows, in
    /// one place:
    ///
    /// * an identifier that is neither an API id nor an ARN names no API at all, so the
    ///   request is malformed rather than pointed at a missing resource;
    /// * the Merged API is resolved in either identifier form, and one that resolves to
    ///   nothing is a not-found;
    /// * an `associationId` names an association OF the Merged API addressed, so one
    ///   that belongs to another Merged API — or to none — is not found under this one.
    pub fn resolve_association(
        &self,
        merged_api_identifier: &str,
        association_id: &str,
    ) -> Result<&SourceApiAssociationRecord, AssociationLookupRefusal> {
        if !is_api_identifier(merged_api_identifier) {
            return Err(AssociationLookupRefusal::BadRequest(invalid_api_id_message(
                merged_api_identifier,
            )));
        }

        let merged = api_by_identifier(&self.graphql_apis, merged_api_identifier).ok_or_else(
            || AssociationLookupRefusal::NotFound(API_NOT_FOUND_MESSAGE.to_string()),
        )?;

        self.source_api_associations
            .get(association_id)
            .filter(|association| association.merged_api_id == merged.api_id)
            .ok_or_else(|| {
                AssociationLookupRefusal::NotFound(ASSOCIATION_NOT_FOUND_MESSAGE.to_string())
            })
    }

    /// Drop every association the API `api_id` takes part in, on either side.
    ///
    /// An association names two APIs, so it cannot outlive either of them: deleting
    /// an API deletes the links it was a side of.
    pub fn remove_api_associations(&mut self, api_id: &str) {
        self.source_api_associations.retain(|_, association| {
            association.source_api_id != api_id && association.merged_api_id != api_id
        });
    }
    /// Mint a fresh opaque id: 26 lowercase base-32 characters, the shape an AppSync
    /// `apiId` takes.
    ///
    /// Drawn from a counter the state advances (seeded from the clock on first use),
    /// never from the request, so creating the same API twice yields two ids.
    pub fn mint_id(&mut self) -> String {
        if self.id_seed == 0 {
            self.id_seed = now_nanos();
        }
        self.id_seed = splitmix64(self.id_seed);
        let low = splitmix64(self.id_seed);
        let mut id = base32_13(self.id_seed);
        id.push_str(&base32_13(low));
        id
    }

    /// Mint a fresh opaque id that can also be addressed as a `ResourceName`: a
    /// [`mint_id`] id whose first character is a letter.
    ///
    /// Every operation that addresses a function (`GetFunction`, `UpdateFunction`,
    /// `DeleteFunction`) takes the `functionId` as a `ResourceName`, whose pattern
    /// `^[_A-Za-z][_0-9A-Za-z]*$` rules out an id starting with a digit, so an id of
    /// that shape would name a function no caller could read back.
    ///
    /// [`mint_id`]: Self::mint_id
    pub fn mint_resource_id(&mut self) -> String {
        // Each turn draws a FRESH id (the counter advances through a bijection), and
        // 26 of the 32 first characters qualify, so the loop ends after a couple of
        // turns rather than retrying one rejected value for ever.
        loop {
            let id = self.mint_id();
            if id.starts_with(|first: char| first.is_ascii_alphabetic()) {
                return id;
            }
        }
    }

    /// The next creation-order number.
    pub fn next_seq(&mut self) -> u64 {
        self.seq_counter = self.seq_counter.wrapping_add(1);
        self.seq_counter
    }
}

/// The number of characters an AppSync API id has.
///
/// The ids this service mints are 26 base-32 characters ([`mint_id`]), the shape
/// AppSync's own `apiId` takes. The model types every `apiId` member as a plain
/// `String`, so nothing refuses a longer one at the boundary; [`is_api_id`] is where
/// this service refuses it.
///
/// [`mint_id`]: ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState::mint_id
pub const API_ID_LEN: usize = 26;

/// Whether `candidate` is syntactically an AppSync API id, i.e. whether it could name
/// an API at all.
///
/// The test is on LENGTH only: an id longer than [`API_ID_LEN`] holds more characters
/// than an AppSync API id has, so it names no API and the call is a bad request, while
/// a shorter one is simply an id this account does not hold and the call is a
/// not-found. The alphabet is deliberately NOT part of the test: AppSync answers
/// not-found for a short id drawn from outside the id alphabet, so a character outside
/// it does not make the request malformed.
pub fn is_api_id(candidate: &str) -> bool {
    !candidate.is_empty() && candidate.chars().count() <= API_ID_LEN
}

/// AppSync's refusal text for an `apiId` that is not an API id ([`is_api_id`]): the
/// rejected id, so the caller can see which value was refused.
pub fn invalid_api_id_message(api_id: &str) -> String {
    format!("Invalid API ID {api_id}")
}

/// `ownerContact`'s documented maximum length: "This field accepts any string input
/// with a length of 0 - 256 characters."
pub const OWNER_CONTACT_MAX_LEN: usize = 256;

/// The name a refusal names the OIDC configuration by.
pub const OPEN_ID_CONNECT_CONFIG: &str = "OpenID Connect Config";

/// The name a refusal names the Cognito user pool configuration by.
pub const USER_POOL_CONFIG: &str = "User Pool Config";

/// The name a refusal names the Lambda authorizer configuration by.
pub const LAMBDA_AUTHORIZER_CONFIG: &str = "Lambda Authorizer Config";

/// Why the authentication configuration a request carries for a GraphQL API is not
/// well formed, as the text a `BadRequestException` (HTTP 400) states it with, or
/// `None` when it is well formed.
///
/// The configuration members of a GraphQL API are the same whether the request
/// CREATES the API or UPDATES it, and so are the grounds on which either request is
/// refused, so the rules are stated here once:
///
/// * an authentication type that is set up through a member of its own
///   ([`missing_auth_config`]) is not configured without that member — checked for
///   the API's own `authenticationType` and, independently, for every
///   `additionalAuthenticationProviders` entry, which the refusal names by its
///   1-based position, the only way a caller can tell them apart;
/// * an authentication type identifies ONE way of authenticating the API, so the API
///   cannot name it twice;
/// * an OIDC provider is identified by its issuer, which "must exactly match the
///   value of `iss` in the ID token" — an absolute URL;
/// * `ownerContact` "accepts any string input with a length of 0 - 256 characters"
///   (its own member documentation), a bound the plain `String` target leaves to the
///   service.
///
/// The per-type presence of the API's OWN user-pool and Lambda configurations is
/// taken as a flag rather than as the structure, because the API-level and the
/// per-provider user-pool configurations are modelled with different shapes
/// (`UserPoolConfig` vs `CognitoUserPoolConfig`) while the rule over them is one.
pub fn graphql_api_config_refusal(
    authentication_type: &str,
    open_id_connect_config: Option<&OpenIDConnectConfig>,
    user_pool_config: bool,
    lambda_authorizer_config: bool,
    additional_authentication_providers: &[AdditionalAuthenticationProvider],
    owner_contact: Option<&str>,
) -> Option<String> {
    if let Some(config) = missing_auth_config(
        Some(authentication_type),
        open_id_connect_config.is_some(),
        user_pool_config,
        lambda_authorizer_config,
    ) {
        return Some(api_level_missing_config_message(config));
    }
    for (position, provider) in additional_authentication_providers.iter().enumerate() {
        if let Some(config) = missing_auth_config(
            provider.authentication_type.as_deref(),
            provider.open_id_connect_config.is_some(),
            provider.user_pool_config.is_some(),
            provider.lambda_authorizer_config.is_some(),
        ) {
            return Some(format!(
                "{config} is missing for additional authentication provider {}",
                position + 1
            ));
        }
    }

    // The set of types already named holds at most one entry per value of the
    // modelled `AuthenticationType` enum, because the request is refused as soon as a
    // type repeats: its size is bounded by the model, not by the length of the list
    // the caller sent, and membership is a set lookup rather than a scan of it.
    let mut specified = std::collections::BTreeSet::from([authentication_type.to_string()]);
    for (position, provider) in additional_authentication_providers.iter().enumerate() {
        let Some(provider_type) = provider.authentication_type.clone() else {
            continue;
        };
        if specified.contains(&provider_type) {
            return Some(format!(
                "Authentication type {provider_type} for additional authentication provider {} \
                 already specified on the API. It can only be specified once.",
                position + 1
            ));
        }
        specified.insert(provider_type);
    }

    for config in open_id_connect_config.into_iter().chain(
        additional_authentication_providers
            .iter()
            .filter_map(|provider| provider.open_id_connect_config.as_ref()),
    ) {
        if !is_absolute_url(&config.issuer) {
            return Some("Issuer is not a valid URL".to_string());
        }
    }

    if owner_contact.is_some_and(|contact| contact.chars().count() > OWNER_CONTACT_MAX_LEN) {
        return Some(format!(
            "ownerContact accepts 0-{OWNER_CONTACT_MAX_LEN} characters"
        ));
    }

    None
}

/// The configuration `authentication_type` is set up through and the request did
/// not carry, or `None` when it carried it (or the type needs none at all:
/// `API_KEY` and `AWS_IAM` are configured by nothing).
///
/// Source: the per-type configuration members of `CreateGraphqlApiRequest`,
/// `UpdateGraphqlApiRequest` and `AdditionalAuthenticationProvider` —
/// `openIDConnectConfig` ("The OIDC configuration"), `userPoolConfig` ("The Amazon
/// Cognito user pool configuration") and `lambdaAuthorizerConfig` ("Configuration
/// for Lambda function authorization").
fn missing_auth_config(
    authentication_type: Option<&str>,
    open_id_connect_config: bool,
    user_pool_config: bool,
    lambda_authorizer_config: bool,
) -> Option<&'static str> {
    match authentication_type? {
        "OPENID_CONNECT" if !open_id_connect_config => Some(OPEN_ID_CONNECT_CONFIG),
        "AMAZON_COGNITO_USER_POOLS" if !user_pool_config => Some(USER_POOL_CONFIG),
        "AWS_LAMBDA" if !lambda_authorizer_config => Some(LAMBDA_AUTHORIZER_CONFIG),
        _ => None,
    }
}

/// How the service states the absence of the API's OWN authentication
/// configuration, per configuration.
///
/// The service does not word these from one template: the recorded refusals state
/// the OIDC configuration's absence as "OpenID Connect Config missing" and the
/// Cognito user pool's as "User Pool Config is empty", so each is stated as the
/// service states it rather than derived from the other. No recording covers the
/// Lambda authorizer configuration, so its absence is worded the way the
/// per-provider refusal words a missing configuration, and the requirement that
/// wording answers is the 400 and `BadRequestException` rather than the text.
fn api_level_missing_config_message(config: &str) -> String {
    match config {
        OPEN_ID_CONNECT_CONFIG => format!("{OPEN_ID_CONNECT_CONFIG} missing"),
        USER_POOL_CONFIG => format!("{USER_POOL_CONFIG} is empty"),
        other => format!("{other} is missing"),
    }
}

/// Whether `candidate` is an absolute URL, as an OIDC issuer has to be.
///
/// Parsed with the `url` crate (WHATWG URL), so the judgement is a real parse
/// rather than a string test: a relative reference, a bare host or a value with no
/// scheme is not one. A scheme-relative or opaque URL (`mailto:…`) carries no host,
/// which an issuer an ID token is checked against must have.
fn is_absolute_url(candidate: &str) -> bool {
    ::url::Url::parse(candidate).is_ok_and(|parsed| parsed.has_host())
}

/// The modelled bounds and alphabet of an `EnvironmentVariableKey`, and whether
/// `candidate` satisfies them.
///
/// Source: the `EnvironmentVariableKey` shape — `@length` 2..64 and `@pattern`
/// `^[A-Za-z]+\w*$` — which `PutGraphqlApiEnvironmentVariables` also states in prose:
/// "Keys must begin with a letter. Keys must be at least two characters long. Keys can
/// only contain letters, numbers, and the underscore character (_)." A map KEY is not
/// checked by the request boundary (only map values and the entry count are), so the
/// service enforces it.
pub const ENVIRONMENT_VARIABLE_KEY_MIN_LEN: usize = 2;

/// The maximum length of an `EnvironmentVariableKey` ([`ENVIRONMENT_VARIABLE_KEY_MIN_LEN`]).
pub const ENVIRONMENT_VARIABLE_KEY_MAX_LEN: usize = 64;

/// Whether `candidate` is a well-formed `EnvironmentVariableKey`: between
/// [`ENVIRONMENT_VARIABLE_KEY_MIN_LEN`] and [`ENVIRONMENT_VARIABLE_KEY_MAX_LEN`]
/// characters, beginning with a letter and otherwise holding only letters, digits and
/// the underscore.
pub fn is_environment_variable_key(candidate: &str) -> bool {
    let length = candidate.chars().count();
    if !(ENVIRONMENT_VARIABLE_KEY_MIN_LEN..=ENVIRONMENT_VARIABLE_KEY_MAX_LEN).contains(&length) {
        return false;
    }
    let mut characters = candidate.chars();
    characters.next().is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// AppSync's refusal text for an `environmentVariables` key that is not an
/// `EnvironmentVariableKey` ([`is_environment_variable_key`]): the rejected key and the
/// constraint it broke, so the caller can fix the request from the message alone.
pub fn invalid_environment_variable_key_message(key: &str) -> String {
    format!(
        "Invalid environment variable key {key}: a key must be between \
         {ENVIRONMENT_VARIABLE_KEY_MIN_LEN} and {ENVIRONMENT_VARIABLE_KEY_MAX_LEN} characters \
         long, must begin with a letter, and can only contain letters, numbers, and the \
         underscore character (_)."
    )
}

/// AppSync's fixed, request-independent text for an API that is absent, carried by
/// `NotFoundException` (HTTP 404) by every operation that resolves an `apiId`.
pub const API_NOT_FOUND_MESSAGE: &str = "API not found.";

/// AppSync's fixed, request-independent text for a `nextToken` a list operation cannot
/// resume from, carried by `BadRequestException` (HTTP 400).
pub const INVALID_PAGINATION_TOKEN_MESSAGE: &str = "Invalid pagination token given.";

/// The text a `NotFoundException` (HTTP 404) carries for an association this account
/// does not hold.
///
/// AppSync's own fixed text for the absent association, as the service answers it when
/// a merge is initiated against an `associationId` that resolves to nothing: it states
/// the absent resource and names nothing of the request that asked for it, so it has no
/// variable part to fill. Shared with `GetSourceApiAssociation`, which resolves the same
/// pair by the same rule.
pub const ASSOCIATION_NOT_FOUND_MESSAGE: &str = "No source API association was found";

/// The text `ConcurrentModificationException` (HTTP 409) carries, which the MODEL
/// states for the shape itself: "Another modification is in progress at this time and
/// it must complete before you can make your change." It is the error's own
/// documentation rather than a per-request detail, so it is request-independent and
/// names nothing of the call that met it.
pub const CONCURRENT_MODIFICATION_MESSAGE: &str =
    "Another modification is in progress at this time and it must complete before you can make \
     your change.";

/// The ARN namespace (and endpoint prefix) this service's resources carry.
pub const ARN_SERVICE: &str = "appsync";

/// The prefix of every AWS ARN, i.e. what tells an `…Identifier` member's ARN form
/// apart from its API-id form.
pub const ARN_PREFIX: &str = "arn:";

/// The `GraphQLApiType` of a standard GraphQL API — one that can be the SOURCE of an
/// association. Source: the `GraphQLApiType` enum and `GraphqlApi$apiType` ("whether
/// the GraphQL API is a standard API (GRAPHQL) or merged API (MERGED)").
pub const GRAPHQL_API_TYPE: &str = "GRAPHQL";

/// The `GraphQLApiType` of a Merged API — the only kind of API source APIs associate
/// WITH. Source: the `GraphQLApiType` enum.
pub const MERGED_API_TYPE: &str = "MERGED";

/// The `MergeType` an association takes when `sourceApiAssociationConfig` names none:
/// "Valid merge types are `MANUAL_MERGE` (default) and `AUTO_MERGE`"
/// (`SourceApiAssociationConfig$mergeType`).
pub const DEFAULT_MERGE_TYPE: &str = "MANUAL_MERGE";

/// The `GraphQLApiIntrospectionConfig` a GraphQL API takes when the request that
/// created or updated it named none: "If no value is provided, the introspection
/// configuration will be set to `ENABLED` by default"
/// (`GraphqlApi$introspectionConfig`).
pub const DEFAULT_INTROSPECTION_CONFIG: &str = "ENABLED";


/// The `SourceApiAssociationStatus` a fresh association carries: creating it schedules
/// the merge of the source API's schema into the Merged API, which has not run yet.
pub const MERGE_SCHEDULED: &str = "MERGE_SCHEDULED";

/// The `SourceApiAssociationStatus` an association reaches once its scheduled merge
/// has been performed.
pub const MERGE_SUCCESS: &str = "MERGE_SUCCESS";

/// The `sourceApiAssociationStatusDetail` that goes with [`MERGE_SCHEDULED`].
pub const MERGE_SCHEDULED_DETAIL: &str =
    "The source API merge is scheduled and has not been performed yet.";

/// The `sourceApiAssociationStatusDetail` that goes with [`MERGE_SUCCESS`].
pub const MERGE_SUCCESS_DETAIL: &str = "The source API was merged into the Merged API.";

/// How long after an association is created the merge it scheduled comes due.
///
/// A chosen span, inside the emulator's 250-750 ms band for a transition the clock
/// owes: AppSync reports a new association in the non-terminal [`MERGE_SCHEDULED`]
/// status, so the span is never zero and a caller reading straight after the create
/// still observes it. Nothing sleeps for it — the due instant is stored on the record
/// and [`SourceApiAssociationRecord::settle`] applies it.
pub const MERGE_SPAN: Duration = Duration::from_millis(500);

/// The `SourceApiAssociationStatus` an association whose deletion has been asked for
/// carries until the deletion has been performed. Source: the
/// `SourceApiAssociationStatus` enum, whose deletion values this is the first of.
pub const DELETION_SCHEDULED: &str = "DELETION_SCHEDULED";

/// The `sourceApiAssociationStatusDetail` that goes with [`DELETION_SCHEDULED`].
pub const DELETION_SCHEDULED_DETAIL: &str =
    "The source API association deletion is scheduled and has not been performed yet.";

/// How long after a disassociate the deletion it scheduled comes due.
///
/// A chosen span, inside the emulator's 250-750 ms band for a transition the clock
/// owes: a disassociate reports the association in the non-terminal
/// [`DELETION_SCHEDULED`] status, so the span is never zero and a caller reading
/// straight after the call still observes it. Nothing sleeps for it — the due instant
/// is stored on the record and [`ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState::settle_associations`]
/// applies it.
pub const DELETION_SPAN: Duration = Duration::from_millis(500);

/// The smallest `ttl` an API cache accepts, in seconds.
///
/// Source: `CreateApiCacheRequest$ttl` and `UpdateApiCacheRequest$ttl`, both
/// documented "TTL in seconds for cache entries. Valid values are 1–3,600 seconds".
/// The members target an unconstrained `Long`, so the window is the service's to
/// enforce rather than the request boundary's.
pub const CACHE_TTL_MIN_SECONDS: i64 = 1;

/// The largest `ttl` an API cache accepts, in seconds
/// ([`CACHE_TTL_MIN_SECONDS`] states the source).
pub const CACHE_TTL_MAX_SECONDS: i64 = 3_600;

/// Whether `ttl` is inside the documented cache-entry window
/// ([`CACHE_TTL_MIN_SECONDS`]..=[`CACHE_TTL_MAX_SECONDS`]).
pub fn is_cache_ttl(ttl: i64) -> bool {
    (CACHE_TTL_MIN_SECONDS..=CACHE_TTL_MAX_SECONDS).contains(&ttl)
}

/// AppSync's refusal text for a `ttl` outside the documented window
/// ([`is_cache_ttl`]): the window itself, built from the two bounds, so the caller can
/// correct the value from the message alone.
pub fn invalid_cache_ttl_message() -> String {
    format!(
        "TTL value cannot be smaller than {CACHE_TTL_MIN_SECONDS} second and larger than \
         {CACHE_TTL_MAX_SECONDS} seconds."
    )
}

/// The `ApiCacheStatus` a cache being provisioned carries: "The instance is currently
/// creating."
pub const CACHE_CREATING: &str = "CREATING";

/// The `ApiCacheStatus` a cache whose settings are being applied carries.
pub const CACHE_MODIFYING: &str = "MODIFYING";

/// The `ApiCacheStatus` a provisioned cache carries: "The instance is available for
/// use."
pub const CACHE_AVAILABLE: &str = "AVAILABLE";

/// How long after a cache is created or updated the instance becomes available.
///
/// A chosen span, inside the emulator's 250-750 ms band for a transition the clock
/// owes: AppSync reports a fresh cache in the non-terminal [`CACHE_CREATING`] status,
/// so the span is never zero and a caller reading straight after the create still
/// observes it. Nothing sleeps for it — the due instant is stored on the record and
/// [`ApiCacheRecord::settle`] applies it.
pub const CACHE_SPAN: Duration = Duration::from_millis(500);

/// The text `NotFoundException` (HTTP 404) carries for an API that holds no cache: the
/// absent resource and the call that would create it.
pub const CACHE_NOT_FOUND_MESSAGE: &str =
    "Unable to get the cache as it doesn't exist, please create the cache first.";

/// The text `BadRequestException` (HTTP 400) carries for a create on an API that
/// already holds a cache: an API holds at most one.
pub const CACHE_ALREADY_EXISTS_MESSAGE: &str =
    "The API already has a cache. Delete the existing cache before creating another one.";

/// The `DataSourceType` of an HTTP endpoint, the one type whose need for a service
/// role depends on how the endpoint is authorized ([`data_source_needs_service_role`]).
pub const HTTP_DATA_SOURCE_TYPE: &str = "HTTP";

/// The `AuthorizationType` an `HttpDataSourceConfig` selects to have AppSync sign its
/// requests to the endpoint with IAM credentials — which it can only do by assuming a
/// role. Source: the `AuthorizationType` enum, whose only value this is.
pub const IAM_AUTHORIZATION_TYPE: &str = "AWS_IAM";

/// Every `DataSourceType` AppSync reaches by assuming the data source's service role.
///
/// Source: `DataSource$type`, which says what each type IS (a Lambda function, a
/// DynamoDB table, an OpenSearch domain, an EventBridge configuration, the Bedrock
/// runtime, a relational database), and `serviceRoleArn` — "The system assumes this
/// role when accessing the data source". The two types left out are the two AppSync
/// reaches without credentials of its own: `NONE` ("There is no data source") and
/// [`HTTP_DATA_SOURCE_TYPE`] (a plain endpoint), the latter only while its
/// `authorizationConfig` does not select [`IAM_AUTHORIZATION_TYPE`].
pub const ROLE_ASSUMING_DATA_SOURCE_TYPES: &[&str] = &[
    "AMAZON_BEDROCK_RUNTIME",
    "AMAZON_DYNAMODB",
    "AMAZON_ELASTICSEARCH",
    "AMAZON_EVENTBRIDGE",
    "AMAZON_OPENSEARCH_SERVICE",
    "AWS_LAMBDA",
    "RELATIONAL_DATABASE",
];

/// Whether these HTTP endpoint settings have AppSync sign its calls to the endpoint
/// with IAM credentials, which it can only do by assuming the data source's service
/// role ([`data_source_needs_service_role`]).
///
/// `false` for a data source that carries no `httpConfig`, and for one whose
/// `authorizationConfig` names another authorization type. Shared by the create and
/// the update, so the two cannot disagree about one configuration.
pub fn selects_iam_authorization(http_config: Option<&HttpDataSourceConfig>) -> bool {
    http_config
        .and_then(|config| config.authorization_config.as_ref())
        .is_some_and(|authorization| {
            authorization.authorization_type == IAM_AUTHORIZATION_TYPE
        })
}

/// Whether AppSync must assume a role to reach a data source of this `type`, i.e.
/// whether `serviceRoleArn` has to be present for the data source to be invocable.
///
/// `iam_authorized` says whether the request's `httpConfig` selects
/// [`IAM_AUTHORIZATION_TYPE`], which is what makes an otherwise credential-free HTTP
/// endpoint need a role; it is ignored for every other type.
pub fn data_source_needs_service_role(data_source_type: &str, iam_authorized: bool) -> bool {
    if data_source_type == HTTP_DATA_SOURCE_TYPE {
        return iam_authorized;
    }
    ROLE_ASSUMING_DATA_SOURCE_TYPES.contains(&data_source_type)
}

/// The text `BadRequestException` (HTTP 400) carries for a data source whose type
/// needs a service role ([`data_source_needs_service_role`]) and whose request carried
/// none: the absent member.
pub const SERVICE_ROLE_NOT_SPECIFIED_MESSAGE: &str = "Service role arn not specified.";

/// Whether `candidate` could identify an API at all: an API id ([`is_api_id`]) or a
/// full resource ARN.
///
/// Those are the two forms every `sourceApiIdentifier` / `mergedApiIdentifier` member
/// documents — "only require the API ID value or ARN of the \[…\] API" — so a value
/// that is neither names no API and the call is a bad request rather than a not-found.
pub fn is_api_identifier(candidate: &str) -> bool {
    candidate.starts_with(ARN_PREFIX) || is_api_id(candidate)
}

/// The stored API `identifier` names, in either accepted form ([`is_api_identifier`]):
/// an ARN is matched against the API's own ARN, anything else against its `apiId`.
/// `None` when this account holds no such API.
pub fn api_by_identifier<'a>(
    apis: &'a BTreeMap<String, GraphqlApiRecord>,
    identifier: &str,
) -> Option<&'a GraphqlApiRecord> {
    if identifier.starts_with(ARN_PREFIX) {
        apis.values().find(|record| record.arn == identifier)
    } else {
        apis.get(identifier)
    }
}

/// The region the caller addressed, read from the request's SigV4 credential scope.
///
/// Every ARN this service mints carries it, so a resource is reported in the region it
/// was created in. Falls back to [`DEFAULT_REGION`] for an unsigned request, which
/// carries no scope to read.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The region an unsigned request is served in, the emulator's default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState>)
}

/// The page size this service serves when a list call names no `maxResults`.
///
/// Both list operations model `maxResults` with a range of 0..25 and a default of
/// `0`, i.e. "unspecified", so an unspecified page is the whole modelled maximum.
pub const DEFAULT_PAGE_SIZE: usize = 25;

/// One page of a list operation: the items to return and the `nextToken` that
/// resumes after them (`None` when the page is the last one).
#[derive(Debug)]
pub struct Page<'a, T> {
    pub items: Vec<&'a T>,
    pub next_token: Option<String>,
}

/// Cut the page `next_token` and `max_results` select out of `ordered`.
///
/// The cursor is the id of the first item of the NEXT page, so it keys on the
/// ordering itself rather than on an index: items created or deleted between two
/// pages never shift an already-returned item into or out of view twice.
/// `None` when `next_token` names no item in `ordered` — a caller that cannot
/// resume answers the list operation's modelled client error.
pub fn paginate<'a, T, K>(
    ordered: &[&'a T],
    key: K,
    next_token: Option<&str>,
    max_results: i32,
) -> Option<Page<'a, T>>
where
    K: Fn(&T) -> &str,
{
    let start = match next_token {
        None => 0,
        Some(token) => ordered.iter().position(|item| key(item) == token)?,
    };
    let page_size = if max_results <= 0 {
        DEFAULT_PAGE_SIZE
    } else {
        max_results as usize
    };
    let end = start.saturating_add(page_size).min(ordered.len());
    Some(Page {
        items: ordered[start..end].to_vec(),
        next_token: ordered
            .get(end)
            .map(|item| key(item).to_string()),
    })
}

/// The API's endpoint URIs, built from the DNS record of each endpoint: the
/// realtime endpoint is a WebSocket URL and every other endpoint an HTTPS one, both
/// addressing the API's `/graphql` path on the hostname `dns` publishes.
fn uris_from_dns(dns: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
    dns.iter()
        .map(|(endpoint, host)| {
            let scheme = if endpoint == REALTIME_ENDPOINT {
                "wss"
            } else {
                "https"
            };
            (endpoint.clone(), format!("{scheme}://{host}/graphql"))
        })
        .collect()
}

/// The key AppSync gives the realtime (WebSocket) endpoint in `dns` and `uris`.
pub const REALTIME_ENDPOINT: &str = "REALTIME";

/// The key AppSync gives the GraphQL (HTTPS) endpoint in `dns` and `uris`.
pub const GRAPHQL_ENDPOINT: &str = "GRAPHQL";

/// The key AppSync gives the HTTP endpoint of an `Api` in its `dns` records:
/// `Api$dns` "will include an HTTP and a real-time endpoint", the realtime one under
/// [`REALTIME_ENDPOINT`].
pub const HTTP_ENDPOINT: &str = "HTTP";

/// The DNS records AppSync publishes for an API it created: one hostname per endpoint,
/// both on the API's own endpoint host label.
///
/// `http_endpoint` is the key the HTTPS endpoint is published under — a GraphQL API
/// publishes it as [`GRAPHQL_ENDPOINT`] (and derives its `uris` from these records),
/// an `Api` as [`HTTP_ENDPOINT`] — so the two creates cannot drift on the hostname
/// shape.
pub fn api_dns(host_label: &str, region: &str, http_endpoint: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            http_endpoint.to_string(),
            format!("{host_label}.appsync-api.{region}.amazonaws.com"),
        ),
        (
            REALTIME_ENDPOINT.to_string(),
            format!("{host_label}.appsync-realtime-api.{region}.amazonaws.com"),
        ),
    ])
}

/// A `BTreeMap` field as the modelled map shape (`HashMap`).
pub fn map_out(map: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
    map.iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Wall-clock nanoseconds since the epoch, used only to seed the id counter.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1
}

/// SplitMix64: a bijection on `u64`, so advancing the id counter through it never
/// revisits a value before the counter wraps.
fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as 13 base-32 characters over the alphabet AppSync's opaque ids are
/// written in: RFC 4648 base-32, lowercased (`a`-`z` and `2`-`7`).
fn base32_13(value: u64) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    (0..13)
        .map(|i| {
            let shift = 5 * (12 - i);
            let digit = (value >> shift) & 0x1f;
            char::from(ALPHABET[digit as usize])
        })
        .collect()
}
