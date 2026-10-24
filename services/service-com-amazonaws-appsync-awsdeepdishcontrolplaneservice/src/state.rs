// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appsync-awsdeepdishcontrolplaneservice service.
//!
//! Holds the GraphQL API lifecycle: every `GraphqlApi` the service created, each with
//! the API keys created under it, and every source-API association between two of
//! them. A handler reaches it through [`service_state`] and answers from what is
//! stored here — nothing is recomputed from the request.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AdditionalAuthenticationProvider, ApiKey, EnhancedMetricsConfig, GraphqlApi,
    LambdaAuthorizerConfig, LogConfig, OpenIDConnectConfig, SourceApiAssociation,
    SourceApiAssociationConfig, UserPoolConfig,
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
    pub created_at: SystemTime,
    /// The API keys created under this API, keyed by key id.
    pub api_keys: BTreeMap<String, ApiKeyRecord>,
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
fn map_out(map: &BTreeMap<String, String>) -> std::collections::HashMap<String, String> {
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
