// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appsync-awsdeepdishcontrolplaneservice service.
//!
//! Holds the GraphQL API lifecycle: every `GraphqlApi` the service created, each with
//! the API keys and data sources created under it, and every source-API association
//! between two of them. A handler reaches it through [`service_state`] and answers from what is
//! stored here — nothing is recomputed from the request.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AdditionalAuthenticationProvider, ApiCache, ApiKey, DataSource, DynamodbDataSourceConfig,
    ElasticsearchDataSourceConfig, EnhancedMetricsConfig, EventBridgeDataSourceConfig, GraphqlApi,
    HttpDataSourceConfig, LambdaAuthorizerConfig, LambdaDataSourceConfig, LogConfig,
    OpenIDConnectConfig, OpenSearchServiceDataSourceConfig, RelationalDatabaseDataSourceConfig,
    SourceApiAssociation, SourceApiAssociationConfig, UserPoolConfig,
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
    /// This record as the modelled `ApiKey` shape.
    pub fn to_api_key(&self) -> ApiKey {
        ApiKey {
            id: Some(self.id.clone()),
            description: self.description.clone(),
            expires: self.expires,
            deletes: self.deletes,
        }
    }
}

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
/// ([`DataSourceRecord::missing_config`]): the member and the type that needs it, so
/// the caller can fix the request from the message alone.
pub fn missing_data_source_config_message(member: &str, data_source_type: &str) -> String {
    format!("{member} is required for a data source of type {data_source_type}")
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
}

impl SourceApiAssociationRecord {
    /// Whether a merge the clock owes this association has not been materialised yet,
    /// i.e. whether a modification of this association is still in progress.
    pub fn merge_pending(&self) -> bool {
        self.merge_due_at.is_some()
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

/// Per-service state for com-amazonaws-appsync-awsdeepdishcontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// Every created GraphQL API, keyed by its minted `apiId`.
    pub graphql_apis: BTreeMap<String, GraphqlApiRecord>,
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
/// does not hold. The requirement the refusal answers is the status and the error
/// shape; no recording pins AppSync's own wording, so this states the absent resource.
pub const ASSOCIATION_NOT_FOUND_MESSAGE: &str = "Source API association not found.";

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
