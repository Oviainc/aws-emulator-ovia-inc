// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appsync-awsdeepdishcontrolplaneservice service.
//!
//! Holds the GraphQL API lifecycle: every `GraphqlApi` the service created, each with
//! the API keys created under it. A handler reaches it through [`service_state`] and
//! answers from what is stored here — nothing is recomputed from the request.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AdditionalAuthenticationProvider, ApiKey, EnhancedMetricsConfig, GraphqlApi,
    LambdaAuthorizerConfig, LogConfig, OpenIDConnectConfig, UserPoolConfig,
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

/// Per-service state for com-amazonaws-appsync-awsdeepdishcontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// Every created GraphQL API, keyed by its minted `apiId`.
    pub graphql_apis: BTreeMap<String, GraphqlApiRecord>,
    /// Advanced once per minted id, so no id is ever a function of the request.
    id_seed: u64,
    /// Advanced once per created resource, giving each record its creation order.
    seq_counter: u64,
}

impl ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
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
