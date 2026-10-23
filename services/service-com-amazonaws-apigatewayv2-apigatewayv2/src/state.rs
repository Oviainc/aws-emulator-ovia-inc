// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigatewayv2-apigatewayv2 service.
//!
//! A single store, [`ComAmazonawsApigatewayv2Apigatewayv2State::apis`], keyed by
//! `ApiId`. An [`Api`] OWNS its child [`Route`]s and [`Integration`]s (and the
//! [`Stage`]s a later slice will fill), so deleting an api cascades to its children in
//! one `remove` and no second map can drift out of step with it.
//!
//! The records here are the service's own, not the wire shapes: a record keeps a
//! value the api always has (its id, its name, its creation instant) as a plain
//! field, and `to_wire` projects it onto the modelled shape the handlers serialize.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types as wire;

/// The alphabet API Gateway mints an `ApiId`/`RouteId` from: lowercase
/// alphanumerics. `Id` is an unconstrained string in the Smithy model
/// (`smithy_query(query="shape", shape="Id")`), so the SHAPE of a minted id is a
/// service convention, not a modelled constraint; its uniqueness within the store IS
/// an invariant, and [`mint_id`] guarantees it by construction.
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// How many characters a minted id carries.
const ID_LEN: u32 = 10;

/// The number of distinct ids [`mint_id`] can render: `36^10`.
const ID_SPACE: u128 = 36u128.pow(ID_LEN);

/// Scrambles consecutive seeds across [`ID_SPACE`]. Prime, so it is coprime with
/// `36^10 = 2^20 * 3^20`, which makes `seed -> seed * MULTIPLIER + OFFSET (mod ID_SPACE)`
/// a BIJECTION: two distinct seeds can never mint the same id, so no collision retry
/// and no random source is needed.
const ID_MULTIPLIER: u128 = 1_000_000_007;

/// The bijection's constant term, so the first id minted is not a run of leading `a`s.
const ID_OFFSET: u128 = 734_291;

/// The `RouteSelectionExpression` an HTTP API gets when the request names none.
///
/// From the `CreateApi$RouteSelectionExpression` doc: "For HTTP APIs, the
/// routeSelectionExpression must be ${request.method} ${request.path}. If not
/// provided, this will be the default for HTTP APIs."
pub const HTTP_ROUTE_SELECTION_EXPRESSION: &str = "$request.method $request.path";

/// The `ApiKeySelectionExpression` an api gets when the request names none; the
/// default AWS documents for API key selection expressions.
pub const DEFAULT_API_KEY_SELECTION_EXPRESSION: &str = "$request.header.x-api-key";

/// The `IpAddressType` an api gets when the request names none. One of
/// `IP_ADDRESS_TYPE_VALUES` (`dualstack | ipv4`); AWS documents IPv4-only as the
/// default for a new API.
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "ipv4";

/// The `AuthorizationType` a route gets when the request names none. One of
/// `AUTHORIZATION_TYPE_VALUES`; `NONE` is the value the `Route$AuthorizationType`
/// doc gives for open access.
pub const DEFAULT_AUTHORIZATION_TYPE: &str = "NONE";

/// The region an unsigned request's resources are reported in — the region every
/// emulator defaults to (see `emulator_server::emulator_arn`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most apis one account may hold, after which CreateApi is refused.
///
/// AWS's published API Gateway quota is 600 HTTP/WebSocket APIs per account per
/// Region, so this is the faithful bound AND the one that stops a loop of CreateApi
/// calls growing this store without end.
pub const MAX_APIS: usize = 600;

/// The most routes one api may hold, after which CreateRoute is refused. AWS's
/// published quota is 300 routes per API.
pub const MAX_ROUTES_PER_API: usize = 300;

/// The most integrations one api may hold, after which CreateIntegration is refused.
/// AWS's published API Gateway quota is 300 integrations per API.
pub const MAX_INTEGRATIONS_PER_API: usize = 300;

/// The most deployments one api may hold, after which CreateDeployment is refused.
///
/// API Gateway publishes a quota of 500 deployments per API, so this is the faithful
/// bound AND what stops a loop of CreateDeployment calls growing this store without
/// end — every call mints a new deployment, so nothing else bounds it.
pub const MAX_DEPLOYMENTS_PER_API: usize = 500;

/// The `DeploymentStatus` a deployment carries once the service has released it, from
/// `DEPLOYMENT_STATUS_VALUES` (`DEPLOYED | FAILED | PENDING`). Deploying an api is
/// synchronous here — the deployment captures the api's current routes and
/// integrations, which this store already holds — so a minted deployment is reported
/// released, the terminal status, and never moves again.
pub const DEPLOYMENT_STATUS_DEPLOYED: &str = "DEPLOYED";

/// The `ConnectionType` an integration gets when the request names none. From
/// `Integration$ConnectionType`: "Specify INTERNET for connections through the public
/// routable internet … The default value is INTERNET."
pub const DEFAULT_CONNECTION_TYPE: &str = "INTERNET";

/// The `TimeoutInMillis` an integration of an HTTP API gets when the request names
/// none: "The default timeout is … 30 seconds for HTTP APIs"
/// (`CreateIntegration$TimeoutInMillis`).
pub const DEFAULT_HTTP_TIMEOUT_MILLIS: i32 = 30_000;

/// The `TimeoutInMillis` an integration of a WebSocket API gets when the request names
/// none: "The default timeout is 29 seconds for WebSocket APIs"
/// (`CreateIntegration$TimeoutInMillis`).
pub const DEFAULT_WEBSOCKET_TIMEOUT_MILLIS: i32 = 29_000;

/// The largest page either paginated operation serves, and the page size it uses when
/// the request names no `MaxResults`.
///
/// A caller-supplied `MaxResults` is clamped DOWN to it, so no single request can walk
/// and serialize the whole store: the work one call costs is bounded server-side, and
/// what does not fit is reported through `NextToken`.
pub const MAX_PAGE_SIZE: usize = 100;

/// Per-service state for com-amazonaws-apigatewayv2-apigatewayv2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApigatewayv2Apigatewayv2State {
    /// Every persisted api, keyed by `ApiId`. `BTreeMap` so every listing walks
    /// the same stable key order, which is what makes `NextToken` paging
    /// resumable.
    pub apis: BTreeMap<String, Api>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_api_id`]; see
    /// [`ID_MULTIPLIER`] for how an id is derived from it.
    pub api_id_seed: u64,
}

impl ComAmazonawsApigatewayv2Apigatewayv2State {
    /// Mint a fresh `ApiId`: ten lowercase alphanumerics, never one already minted.
    pub fn mint_api_id(&mut self) -> String {
        self.api_id_seed = self.api_id_seed.wrapping_add(1);
        mint_id(self.api_id_seed)
    }
}

/// A persisted `Api` resource.
#[derive(Debug, Clone)]
pub struct Api {
    pub api_id: String,
    pub name: String,
    /// One of `PROTOCOL_TYPE_VALUES` (`HTTP | WEBSOCKET`).
    pub protocol_type: String,
    pub api_endpoint: String,
    pub description: Option<String>,
    pub version: Option<String>,
    /// Required on the modelled `Api` shape, so the record always carries one.
    pub route_selection_expression: String,
    pub api_key_selection_expression: String,
    pub cors_configuration: Option<wire::Cors>,
    pub disable_execute_api_endpoint: bool,
    pub disable_schema_validation: bool,
    pub ip_address_type: String,
    pub created_date: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// This api's routes, keyed by `RouteId`.
    pub routes: BTreeMap<String, Route>,
    /// Drives [`Api::mint_route_id`]. Per-api, because a `RouteId` only has to be
    /// unique within the api that owns the route.
    pub route_id_seed: u64,
    /// This api's integrations, keyed by `IntegrationId`.
    pub integrations: BTreeMap<String, Integration>,
    /// Drives [`Api::mint_integration_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: an `IntegrationId` is unique within its api.
    pub integration_id_seed: u64,
    /// This api's stages, keyed by `StageName`. Reserved for the stage slice.
    pub stages: BTreeMap<String, Stage>,
    /// This api's deployments, keyed by `DeploymentId`.
    pub deployments: BTreeMap<String, Deployment>,
    /// Drives [`Api::mint_deployment_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: a `DeploymentId` is unique within its api.
    pub deployment_id_seed: u64,
}

impl Api {
    /// Mint a fresh `RouteId` for this api: ten lowercase alphanumerics, never one
    /// this api already minted.
    pub fn mint_route_id(&mut self) -> String {
        self.route_id_seed = self.route_id_seed.wrapping_add(1);
        mint_id(self.route_id_seed)
    }

    /// Mint a fresh `IntegrationId` for this api: ten lowercase alphanumerics, never
    /// one this api already minted.
    pub fn mint_integration_id(&mut self) -> String {
        self.integration_id_seed = self.integration_id_seed.wrapping_add(1);
        mint_id(self.integration_id_seed)
    }

    /// Mint a fresh `DeploymentId` for this api: ten lowercase alphanumerics, never
    /// one this api already minted.
    pub fn mint_deployment_id(&mut self) -> String {
        self.deployment_id_seed = self.deployment_id_seed.wrapping_add(1);
        mint_id(self.deployment_id_seed)
    }
}

impl Api {
    /// Project the record onto the modelled `Api` shape.
    pub fn to_wire(&self) -> wire::Api {
        wire::Api {
            api_endpoint: Some(self.api_endpoint.clone()),
            // Only a resource another service created on the caller's behalf is
            // API-Gateway-managed; an api created through CreateApi is not.
            api_gateway_managed: Some(false),
            api_id: Some(self.api_id.clone()),
            api_key_selection_expression: Some(self.api_key_selection_expression.clone()),
            cors_configuration: self.cors_configuration.clone(),
            created_date: Some(to_timestamp(self.created_date)),
            description: self.description.clone(),
            disable_schema_validation: Some(self.disable_schema_validation),
            disable_execute_api_endpoint: Some(self.disable_execute_api_endpoint),
            // `ImportInfo`/`Warnings` report on an OpenAPI import. Nothing was
            // imported into this record, so there is nothing to report.
            import_info: None,
            ip_address_type: Some(self.ip_address_type.clone()),
            name: self.name.clone(),
            protocol_type: self.protocol_type.clone(),
            route_selection_expression: self.route_selection_expression.clone(),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            version: self.version.clone(),
            warnings: None,
        }
    }
}

/// A persisted `Route` resource, owned by its parent [`Api`].
#[derive(Debug, Clone, Default)]
pub struct Route {
    pub route_id: String,
    pub route_key: String,
    pub target: Option<String>,
    /// One of `AUTHORIZATION_TYPE_VALUES`.
    pub authorization_type: String,
    pub authorizer_id: Option<String>,
    pub api_key_required: bool,
    pub operation_name: Option<String>,
    pub authorization_scopes: Vec<String>,
    pub model_selection_expression: Option<String>,
    pub route_response_selection_expression: Option<String>,
    pub request_models: BTreeMap<String, String>,
    pub request_parameters: BTreeMap<String, wire::ParameterConstraints>,
    /// True only for the `$default` route quick create mints alongside the api:
    /// "If you created an API using quick create, the $default route is managed by
    /// API Gateway" (`Route$ApiGatewayManaged`).
    pub api_gateway_managed: bool,
}

impl Route {
    /// Project the record onto the modelled `Route` shape.
    pub fn to_wire(&self) -> wire::Route {
        wire::Route {
            api_gateway_managed: Some(self.api_gateway_managed),
            api_key_required: Some(self.api_key_required),
            authorization_scopes: Some(self.authorization_scopes.clone()),
            authorization_type: Some(self.authorization_type.clone()),
            authorizer_id: self.authorizer_id.clone(),
            model_selection_expression: self.model_selection_expression.clone(),
            operation_name: self.operation_name.clone(),
            request_models: Some(
                self.request_models
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            request_parameters: Some(
                self.request_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            route_id: Some(self.route_id.clone()),
            route_key: self.route_key.clone(),
            route_response_selection_expression: self.route_response_selection_expression.clone(),
            target: self.target.clone(),
        }
    }
}

/// A persisted `Integration` resource, owned by its parent [`Api`].
///
/// The record keeps what the create resolved — the id it minted, the connection type and
/// the timeout the request left to their documented defaults — so every reader of the
/// integration reports one value, and [`Integration::to_wire`] projects it onto the
/// modelled shape the handlers serialize.
#[derive(Debug, Clone, Default)]
pub struct Integration {
    pub integration_id: String,
    /// One of `INTEGRATION_TYPE_VALUES` (`AWS | AWS_PROXY | HTTP | HTTP_PROXY | MOCK`).
    pub integration_type: String,
    pub integration_uri: Option<String>,
    pub integration_method: Option<String>,
    pub integration_subtype: Option<String>,
    /// One of `CONNECTION_TYPE_VALUES`; [`DEFAULT_CONNECTION_TYPE`] when the request
    /// named none.
    pub connection_type: String,
    pub connection_id: Option<String>,
    pub content_handling_strategy: Option<String>,
    pub credentials_arn: Option<String>,
    pub description: Option<String>,
    pub passthrough_behavior: Option<String>,
    pub payload_format_version: Option<String>,
    pub request_parameters: BTreeMap<String, String>,
    pub request_templates: BTreeMap<String, String>,
    pub response_parameters: BTreeMap<String, BTreeMap<String, String>>,
    pub template_selection_expression: Option<String>,
    /// The documented default for the parent api's protocol when the request named no
    /// timeout.
    pub timeout_in_millis: i32,
    pub tls_config: Option<wire::TlsConfig>,
    /// True only for an integration another operation created on the caller's behalf:
    /// "If you created an API using quick create, the resulting integration is managed
    /// by API Gateway" (`Integration$ApiGatewayManaged`).
    pub api_gateway_managed: bool,
}

impl Integration {
    /// Project the record onto the modelled `Integration` shape.
    pub fn to_wire(&self) -> wire::Integration {
        wire::Integration {
            api_gateway_managed: Some(self.api_gateway_managed),
            connection_id: self.connection_id.clone(),
            connection_type: Some(self.connection_type.clone()),
            content_handling_strategy: self.content_handling_strategy.clone(),
            credentials_arn: self.credentials_arn.clone(),
            description: self.description.clone(),
            integration_id: Some(self.integration_id.clone()),
            integration_method: self.integration_method.clone(),
            // The integration response selection expression is a WebSocket-only value
            // the service computes from the integration's own responses; nothing here
            // has any, so there is none to report.
            integration_response_selection_expression: None,
            integration_subtype: self.integration_subtype.clone(),
            integration_type: Some(self.integration_type.clone()),
            integration_uri: self.integration_uri.clone(),
            passthrough_behavior: self.passthrough_behavior.clone(),
            payload_format_version: self.payload_format_version.clone(),
            request_parameters: Some(
                self.request_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            request_templates: Some(
                self.request_templates
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            response_parameters: Some(
                self.response_parameters
                    .iter()
                    .map(|(selection, parameters)| {
                        (
                            selection.clone(),
                            parameters
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            template_selection_expression: self.template_selection_expression.clone(),
            timeout_in_millis: Some(self.timeout_in_millis),
            tls_config: self.tls_config.clone(),
        }
    }
}

/// A persisted `Stage`. Reserved so the stage operations can land without a state
/// rewrite; nothing creates one yet, but [`Deployment`] already points stages at the
/// deployment they serve.
#[derive(Debug, Clone, Default)]
pub struct Stage {
    pub stage_name: String,
    pub description: Option<String>,
    pub auto_deploy: bool,
    pub tags: BTreeMap<String, String>,
    /// The `DeploymentId` this stage serves, set by the create that deployed to it:
    /// "A Deployment must be associated with a Stage for it to be callable over the
    /// internet" (the `Deployment` shape's own doc).
    pub deployment_id: Option<String>,
}

/// A persisted `Deployment` resource, owned by its parent [`Api`].
///
/// "An immutable representation of an API that can be called by users" (the
/// `Deployment` shape doc), so nothing but its description is ever rewritten.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub deployment_id: String,
    pub description: Option<String>,
    /// One of `DEPLOYMENT_STATUS_VALUES`.
    pub deployment_status: String,
    pub deployment_status_message: Option<String>,
    /// True only for a deployment the service released by itself, for a stage whose
    /// `AutoDeploy` is on: "Specifies whether a deployment was automatically released"
    /// (`Deployment$AutoDeployed`). A deployment the caller asked for is not one.
    pub auto_deployed: bool,
    pub created_date: SystemTime,
}

impl Deployment {
    /// Project the record onto the modelled `Deployment` shape.
    pub fn to_wire(&self) -> wire::Deployment {
        wire::Deployment {
            auto_deployed: Some(self.auto_deployed),
            created_date: Some(to_timestamp(self.created_date)),
            deployment_id: Some(self.deployment_id.clone()),
            deployment_status: Some(self.deployment_status.clone()),
            deployment_status_message: self.deployment_status_message.clone(),
            description: self.description.clone(),
        }
    }
}

/// Render a stored instant as the modelled `__timestampIso8601` member.
///
/// Truncated to whole seconds: `createdDate` is rendered as an ISO-8601
/// `date-time` and the service reports it at second granularity.
fn to_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    ::aws_smithy_types::DateTime::from_secs(i64::try_from(secs).unwrap_or(i64::MAX))
}

/// Mint one id from `seed`: [`ID_LEN`] characters drawn from [`ID_ALPHABET`].
///
/// `seed -> seed * ID_MULTIPLIER + ID_OFFSET (mod ID_SPACE)` is a bijection (see
/// [`ID_MULTIPLIER`]), so consecutive seeds give unpredictable-looking ids that never
/// repeat. That makes uniqueness structural: no collision retry, no exhaustion branch
/// and no random source.
fn mint_id(seed: u64) -> String {
    let mut scrambled = (u128::from(seed) * ID_MULTIPLIER + ID_OFFSET) % ID_SPACE;
    let mut id = Vec::with_capacity(ID_LEN as usize);
    for _ in 0..ID_LEN {
        let digit = usize::try_from(scrambled % 36).unwrap_or(0);
        id.push(ID_ALPHABET[digit]);
        scrambled /= 36;
    }
    String::from_utf8(id).unwrap_or_default()
}

/// The region this request is scoped to: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] for a request that carries no scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The default execute-api endpoint of an api: the host clients invoke it on when
/// no custom domain name is configured.
///
/// From `Api$DisableExecuteApiEndpoint`: "clients can invoke your API with the
/// default https://{api_id}.execute-api.{region}.amazonaws.com endpoint".
pub fn execute_api_endpoint(api_id: &str, region: &str) -> String {
    format!("https://{api_id}.execute-api.{region}.amazonaws.com")
}

/// Resolve a `MaxResults` query value into the page size to serve.
///
/// `MaxResults` is modelled as `__string` on these operations, so the numeric rule is
/// the handler's to enforce: a positive decimal integer. The result is clamped to
/// [`MAX_PAGE_SIZE`], which is also the page size an absent `MaxResults` gets, so the
/// page size is always bounded server-side rather than by what the caller asked for.
/// `Err` carries the message for the operation's `BadRequestException`.
pub fn parse_max_results(raw: Option<&str>) -> Result<usize, String> {
    let Some(raw) = raw else {
        return Ok(MAX_PAGE_SIZE);
    };
    match raw.parse::<i64>() {
        Ok(n) if n >= 1 => Ok(usize::try_from(n).unwrap_or(MAX_PAGE_SIZE).min(MAX_PAGE_SIZE)),
        _ => Err(format!(
            "Invalid value for MaxResults: '{raw}'. MaxResults must be a positive integer."
        )),
    }
}

/// One page of a key-ordered store.
///
/// Walks `items` in key order from `next_token` (inclusive) and takes at most
/// `max_results`. The returned token is the key of the first item that did NOT
/// fit, so it is present only when more remain and resuming from it continues
/// exactly where this page stopped.
pub fn page<'a, V>(
    items: &'a BTreeMap<String, V>,
    next_token: Option<&str>,
    max_results: usize,
) -> (Vec<&'a V>, Option<String>) {
    // `range` seeks straight to the resume key, so a full paginated walk costs one
    // traversal rather than one scan per page.
    let start = match next_token {
        Some(token) => ::std::ops::Bound::Included(token),
        None => ::std::ops::Bound::Unbounded,
    };
    let mut rest = items.range::<str, _>((start, ::std::ops::Bound::Unbounded));
    let page: Vec<&V> = rest
        .by_ref()
        .take(max_results)
        .map(|(_, value)| value)
        .collect();
    let token = rest.next().map(|(key, _)| key.clone());
    (page, token)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApigatewayv2Apigatewayv2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApigatewayv2Apigatewayv2State>)
}
