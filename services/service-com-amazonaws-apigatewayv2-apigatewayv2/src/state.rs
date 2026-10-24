// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apigatewayv2-apigatewayv2 service.
//!
//! A single store, [`ComAmazonawsApigatewayv2Apigatewayv2State::apis`], keyed by
//! `ApiId`. An [`Api`] OWNS its child [`Route`]s, [`Integration`]s and [`Stage`]s, so
//! deleting an api cascades to its children in one `remove` and no second map can
//! drift out of step with it.
//!
//! The records here are the service's own, not the wire shapes: a record keeps a
//! value the api always has (its id, its name, its creation instant) as a plain
//! field, and `to_wire` projects it onto the modelled shape the handlers serialize.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

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

/// The `ProtocolType` value that selects a WebSocket API, from
/// `PROTOCOL_TYPE_VALUES` (`HTTP | WEBSOCKET`) — the protocol whose two-way
/// communication the route-response resources belong to, and the one whose
/// `ApiEndpoint` carries the WebSocket scheme (see [`execute_api_scheme`]).
pub const WEBSOCKET_PROTOCOL_TYPE: &str = "WEBSOCKET";

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

/// The most route responses one route may hold, after which CreateRouteResponse is
/// refused.
///
/// AWS publishes no separate route-responses-per-route quota, so this is a CHOSEN
/// bound, set to the per-parent quota this service already enforces for routes
/// ([`MAX_ROUTES_PER_API`]): what it buys is that no loop of CreateRouteResponse calls
/// can grow this store without end, and that the per-route duplicate-key scan stays
/// bounded. `TooManyRequestsException` ("A limit has been exceeded") is the modelled
/// error the operation refuses with.
pub const MAX_ROUTE_RESPONSES_PER_ROUTE: usize = 300;

/// The most models one api may hold, after which CreateModel is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: API Gateway publishes per-api
/// quotas for the other child resources of an api (300 routes, 300 integrations) and
/// this takes the same bound for models, so no loop of CreateModel calls can grow this
/// store without end. Cite the routes/integrations quotas for those; do not cite this
/// as a published models quota.
pub const MAX_MODELS_PER_API: usize = 300;

/// The most custom domain names one account may hold in one Region, after which
/// CreateDomainName is refused.
///
/// API Gateway publishes a quota of 120 custom domain names per account per Region, so
/// this is the faithful bound AND what stops a loop of CreateDomainName calls growing
/// this store without end. `TooManyRequestsException` ("A limit has been exceeded") is
/// the modelled error the operation refuses with.
pub const MAX_DOMAIN_NAMES: usize = 120;

/// The longest custom domain name this service accepts.
///
/// `DomainName$DomainName` targets `StringWithLengthBetween1And512`, whose own
/// documentation is "A string with a length between [1-512]"
/// (`smithy_query(query="shape", shape="StringWithLengthBetween1And512")`). The shape
/// carries no `@length` trait, so the bound is a handler's to enforce rather than the
/// boundary validator's.
pub const DOMAIN_NAME_MAX_LENGTH: usize = 512;

/// The service signing name this emulator's ARNs are built under — the `apigateway`
/// signing name and ARN namespace this model shares with the API Gateway control plane
/// (`smithy_query(query="service_info")`).
pub const ARN_SERVICE: &str = "apigateway";

/// The `RoutingMode` a domain name gets when the request names none, out of
/// `ROUTING_MODE_VALUES` (`API_MAPPING_ONLY | ROUTING_RULE_ONLY |
/// ROUTING_RULE_THEN_API_MAPPING`).
///
/// A CHOSEN default, not a figure AWS publishes: the model documents the member only as
/// "The routing mode", so this takes the one value that needs no routing rule to resolve
/// a request — a domain name reached through its `ApiMapping`s alone, which is what a
/// domain name created without any routing rule can actually serve.
pub const DEFAULT_ROUTING_MODE: &str = "API_MAPPING_ONLY";

/// The `EndpointType` a domain name configuration gets when the request names none, out
/// of `ENDPOINT_TYPE_VALUES` (`EDGE | REGIONAL`).
///
/// Every operation of this model is bound under `/v2/…` and every api it creates is
/// reported on a REGIONAL `{api-id}.execute-api.{region}.amazonaws.com` host
/// (`CreateApiResponse$ApiEndpoint`, see [`execute_api_endpoint`]), so `REGIONAL` is the
/// endpoint type a domain name fronting one of them carries; `EDGE` is the
/// edge-optimized form the member's certificate wording belongs to.
pub const DEFAULT_ENDPOINT_TYPE: &str = "REGIONAL";

/// The `SecurityPolicy` a domain name configuration gets when the request names none,
/// out of `SECURITY_POLICY_VALUES` (`TLS_1_0 | TLS_1_2`).
///
/// A CHOSEN default, not a figure AWS publishes: `DomainNameConfiguration$SecurityPolicy`
/// documents only that "The valid values are TLS_1_0 and TLS_1_2", so this takes the
/// later TLS version of the two, which is the one a domain name minted today can
/// actually be reached over.
pub const DEFAULT_SECURITY_POLICY: &str = "TLS_1_2";

/// The `ApiMappingSelectionExpression` every domain name reports.
///
/// An `ApiMapping` selects an api by the base path of the incoming request
/// (`CreateApiMapping$ApiMappingKey`), so the expression that extracts the value the
/// mapping is matched on is the request's base path. It is service-computed: no member
/// of any request sets it.
pub const API_MAPPING_SELECTION_EXPRESSION: &str = "$request.basepath";

/// The `DomainNameStatus` of a domain name whose endpoint migration has finished, out of
/// `DOMAIN_NAME_STATUS_VALUES`. From `DomainNameConfiguration$DomainNameStatus`: "If it
/// is AVAILABLE, the domain can be updated."
pub const DOMAIN_NAME_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `DomainNameStatus` of a domain name whose endpoint migration is still in flight,
/// out of `DOMAIN_NAME_STATUS_VALUES`. From
/// `DomainNameConfiguration$DomainNameStatus`: "If the status is UPDATING, the domain
/// cannot be modified further until the existing operation is complete."
pub const DOMAIN_NAME_STATUS_UPDATING: &str = "UPDATING";

/// How long a domain name's endpoint migration takes before the clock owes it
/// `AVAILABLE` again.
///
/// A CHOSEN span, not one AWS documents: AWS takes as long as propagating a certificate
/// to its endpoints takes. What matters is that it is never zero — a caller that updates
/// a domain name's `DomainNameConfigurations` and immediately reads it back still sees
/// the `UPDATING` the model documents, and the refusal that status carries — and that
/// nothing sleeps for it: the due instant is stored and the status is derived from the
/// clock on every read.
pub const DOMAIN_NAME_MIGRATION: Duration = Duration::from_millis(500);

/// The most authorizers one api may hold, after which CreateAuthorizer is refused.
///
/// API Gateway publishes a quota of 10 authorizers per API, so this is the faithful
/// bound AND what stops a loop of CreateAuthorizer calls growing this store without
/// end — every call mints a new authorizer, so nothing else bounds it.
pub const MAX_AUTHORIZERS_PER_API: usize = 10;

/// The `AuthorizerType` value that selects a JWT authorizer, from the generated
/// `AUTHORIZER_TYPE_VALUES` catalog (`JWT | REQUEST`); the integration test
/// `the_authorizer_type_names_are_the_modelled_catalog` pins these two names against
/// that catalog so the spelling here cannot drift from the model. From
/// `CreateAuthorizer$AuthorizerType`: "Specify JWT to use JSON Web Tokens (supported
/// only for HTTP APIs)."
pub const AUTHORIZER_TYPE_JWT: &str = "JWT";

/// The `AuthorizerType` value that selects a Lambda authorizer taking the incoming
/// request, from the generated `AUTHORIZER_TYPE_VALUES` catalog (`JWT | REQUEST`). From
/// `CreateAuthorizer$AuthorizerType`: "Specify REQUEST for a Lambda function using
/// incoming request parameters."
pub const AUTHORIZER_TYPE_REQUEST: &str = "REQUEST";

/// The `AuthorizerPayloadFormatVersion` values the service accepts.
///
/// `StringWithLengthBetween1And64` carries no pattern, so the catalog is the member's
/// own documentation rather than a modelled constraint
/// (`CreateAuthorizer$AuthorizerPayloadFormatVersion`: "Supported values are 1.0 and
/// 2.0").
pub const AUTHORIZER_PAYLOAD_FORMAT_VERSIONS: &[&str] = &["1.0", "2.0"];

/// The `AuthorizationType` values that hand a route's authorization decision to an
/// `Authorizer` resource, out of the generated `AUTHORIZATION_TYPE_VALUES` catalog
/// (`AWS_IAM | CUSTOM | JWT | NONE`).
///
/// From `Route$AuthorizationType`: "JWT for using JSON Web Tokens" and "CUSTOM for
/// using a Lambda authorizer" — both name a resource, and `Route$AuthorizerId` is "the
/// identifier of the Authorizer resource to be associated with this route". The other
/// two decide without one: `NONE` is "open access" and `AWS_IAM` uses "AWS IAM
/// permissions".
pub const AUTHORIZER_BACKED_AUTHORIZATION_TYPES: &[&str] = &["CUSTOM", "JWT"];

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

/// The `ProtocolType` value that selects an HTTP API, from `PROTOCOL_TYPE_VALUES`
/// (`smithy_query(query="shape", shape="ProtocolType")`: `HTTP | WEBSOCKET`).
pub const HTTP_PROTOCOL_TYPE: &str = "HTTP";

/// The `ConnectionType` an integration gets when the request names none. From
/// `Integration$ConnectionType`: "Specify INTERNET for connections through the public
/// routable internet … The default value is INTERNET."
pub const DEFAULT_CONNECTION_TYPE: &str = "INTERNET";

/// The `IntegrationType` values only a WebSocket api supports, drawn from the member's
/// own documentation rather than from a hand-written allowlist.
///
/// `CreateIntegration$IntegrationType` documents each value of
/// `INTEGRATION_TYPE_VALUES` (`AWS | AWS_PROXY | HTTP | HTTP_PROXY | MOCK`) and marks
/// exactly three "Supported only for WebSocket APIs": "AWS: for integrating the route
/// or method request with an AWS service action … Supported only for WebSocket APIs",
/// "HTTP: for integrating the route or method request with an HTTP endpoint … Supported
/// only for WebSocket APIs" and "MOCK: for integrating the route or method request with
/// API Gateway as a 'loopback' endpoint without invoking any backend. Supported only for
/// WebSocket APIs". The two proxy values carry no such clause, so they are what is left
/// for an HTTP api.
pub const WEBSOCKET_ONLY_INTEGRATION_TYPES: &[&str] = &["AWS", "HTTP", "MOCK"];

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
    /// Every persisted custom domain name, keyed by its `DomainName` — the name IS the
    /// resource's identifier on the wire (`GET|PATCH|DELETE /v2/domainnames/{DomainName}`),
    /// so there is no second id to key on. `BTreeMap` for the same stable key order the
    /// api store relies on.
    pub domain_names: BTreeMap<String, DomainName>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_domain_name_id`]; see
    /// [`ID_MULTIPLIER`] for how an id is derived from it.
    pub domain_name_id_seed: u64,
}

impl ComAmazonawsApigatewayv2Apigatewayv2State {
    /// Mint a fresh `ApiId`: ten lowercase alphanumerics, never one already minted.
    pub fn mint_api_id(&mut self) -> String {
        self.api_id_seed = self.api_id_seed.wrapping_add(1);
        mint_id(self.api_id_seed)
    }

    /// Mint the id of the regional host a custom domain name is served on: ten
    /// lowercase alphanumerics, never one already minted. Drawn from a seed of its own,
    /// so minting a domain name never shifts the ids apis get.
    pub fn mint_domain_name_id(&mut self) -> String {
        self.domain_name_id_seed = self.domain_name_id_seed.wrapping_add(1);
        mint_id(self.domain_name_id_seed)
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
    /// This api's stages, keyed by `StageName`.
    pub stages: BTreeMap<String, Stage>,
    /// This api's models, keyed by `ModelId`.
    pub models: BTreeMap<String, Model>,
    /// Drives [`Api::mint_model_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: a `ModelId` is unique within its api.
    pub model_id_seed: u64,
    /// This api's deployments, keyed by `DeploymentId`.
    pub deployments: BTreeMap<String, Deployment>,
    /// Drives [`Api::mint_deployment_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: a `DeploymentId` is unique within its api.
    pub deployment_id_seed: u64,
    /// This api's authorizers, keyed by `AuthorizerId`.
    pub authorizers: BTreeMap<String, Authorizer>,
    /// Drives [`Api::mint_authorizer_id`]. Per-api, for the same reason as
    /// [`Api::route_id_seed`]: an `AuthorizerId` is unique within its api.
    pub authorizer_id_seed: u64,
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

    /// Mint a fresh `ModelId` for this api: ten lowercase alphanumerics, never one
    /// this api already minted.
    pub fn mint_model_id(&mut self) -> String {
        self.model_id_seed = self.model_id_seed.wrapping_add(1);
        mint_id(self.model_id_seed)
    }

    /// Mint a fresh `DeploymentId` for this api: ten lowercase alphanumerics, never
    /// one this api already minted.
    pub fn mint_deployment_id(&mut self) -> String {
        self.deployment_id_seed = self.deployment_id_seed.wrapping_add(1);
        mint_id(self.deployment_id_seed)
    }

    /// Mint a fresh `AuthorizerId` for this api: ten lowercase alphanumerics, never
    /// one this api already minted.
    pub fn mint_authorizer_id(&mut self) -> String {
        self.authorizer_id_seed = self.authorizer_id_seed.wrapping_add(1);
        mint_id(self.authorizer_id_seed)
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
    /// This route's route responses, keyed by `RouteResponseId`. Owned by the route,
    /// so deleting the route (or its api) cascades to them in one `remove`.
    pub route_responses: BTreeMap<String, RouteResponse>,
    /// Drives [`Route::mint_route_response_id`]. Per-route, because a
    /// `RouteResponseId` only has to be unique within the route that owns the
    /// response.
    pub route_response_id_seed: u64,
}

impl Route {
    /// Mint a fresh `RouteResponseId` for this route: ten lowercase alphanumerics,
    /// never one this route already minted.
    pub fn mint_route_response_id(&mut self) -> String {
        self.route_response_id_seed = self.route_response_id_seed.wrapping_add(1);
        mint_id(self.route_response_id_seed)
    }

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

/// A persisted `RouteResponse` resource, owned by its parent [`Route`].
///
/// A route response is WebSocket-only: `RouteResponse$ModelSelectionExpression` is
/// "Supported only for WebSocket APIs", and the selection keys it is matched against
/// are the WebSocket API's route response selection expressions.
#[derive(Debug, Clone, Default)]
pub struct RouteResponse {
    pub route_response_id: String,
    /// Required on the modelled `RouteResponse` shape, so the record always carries
    /// one. Unique within the owning route.
    pub route_response_key: String,
    pub model_selection_expression: Option<String>,
    pub response_models: BTreeMap<String, String>,
    pub response_parameters: BTreeMap<String, wire::ParameterConstraints>,
}

impl RouteResponse {
    /// Project the record onto the modelled `RouteResponse` shape.
    pub fn to_wire(&self) -> wire::RouteResponse {
        wire::RouteResponse {
            model_selection_expression: self.model_selection_expression.clone(),
            response_models: Some(
                self.response_models
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            response_parameters: Some(
                self.response_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            route_response_id: Some(self.route_response_id.clone()),
            route_response_key: self.route_response_key.clone(),
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

/// A persisted `Model` resource, owned by its parent [`Api`].
///
/// The record keeps the schema SOURCE the create accepted, verbatim: every reader of
/// the model reports the document the caller supplied, and [`Model::to_wire`] projects
/// the record onto the modelled shape the handlers serialize.
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub model_id: String,
    pub name: String,
    pub content_type: Option<String>,
    pub description: Option<String>,
    pub schema: String,
}

impl Model {
    /// Project the record onto the modelled `Model` shape.
    pub fn to_wire(&self) -> wire::Model {
        wire::Model {
            content_type: self.content_type.clone(),
            description: self.description.clone(),
            model_id: Some(self.model_id.clone()),
            name: self.name.clone(),
            schema: Some(self.schema.clone()),
        }
    }
}

/// A persisted `Authorizer` resource, owned by its parent [`Api`].
///
/// The record keeps the configuration the create accepted, so every reader of the
/// authorizer reports one record rather than its own reconstruction of it, and
/// [`Authorizer::to_wire`] projects it onto the modelled shape the handlers serialize.
#[derive(Debug, Clone, Default)]
pub struct Authorizer {
    pub authorizer_id: String,
    /// Required on the modelled `Authorizer` shape, so the record always carries one.
    pub name: String,
    /// One of `AUTHORIZER_TYPE_VALUES` (`JWT | REQUEST`).
    pub authorizer_type: String,
    pub authorizer_uri: Option<String>,
    pub authorizer_credentials_arn: Option<String>,
    pub authorizer_payload_format_version: Option<String>,
    pub authorizer_result_ttl_in_seconds: Option<i32>,
    pub enable_simple_responses: Option<bool>,
    /// `IdentitySource` is a REQUIRED member of `CreateAuthorizerRequest`, so the
    /// record always carries the list — possibly empty, which a REQUEST authorizer
    /// with caching disabled is allowed to be.
    pub identity_source: Vec<String>,
    pub identity_validation_expression: Option<String>,
    pub jwt_configuration: Option<wire::JWTConfiguration>,
}

impl Authorizer {
    /// Project the record onto the modelled `Authorizer` shape.
    pub fn to_wire(&self) -> wire::Authorizer {
        wire::Authorizer {
            authorizer_credentials_arn: self.authorizer_credentials_arn.clone(),
            authorizer_id: Some(self.authorizer_id.clone()),
            authorizer_payload_format_version: self.authorizer_payload_format_version.clone(),
            authorizer_result_ttl_in_seconds: self.authorizer_result_ttl_in_seconds,
            authorizer_type: Some(self.authorizer_type.clone()),
            authorizer_uri: self.authorizer_uri.clone(),
            enable_simple_responses: self.enable_simple_responses,
            // The list the record holds, reported whether or not it is empty: the
            // member is required on the request, so the resource always has one.
            identity_source: Some(self.identity_source.clone()),
            identity_validation_expression: self.identity_validation_expression.clone(),
            jwt_configuration: self.jwt_configuration.clone(),
            name: self.name.clone(),
        }
    }
}

/// `Ok(())`, or the reason this configuration is not one its own `AuthorizerType`
/// admits — the message for the caller's `BadRequestException`.
///
/// Every clause is a member's own documentation, which makes a member required for one
/// authorizer type, or one api protocol, and undefined for another:
///
/// * `AuthorizerType` — "Specify JWT to use JSON Web Tokens (supported only for HTTP
///   APIs)", so a WebSocket api takes only `REQUEST` authorizers.
/// * `JwtConfiguration` — "Required for the JWT authorizer type", and its `Issuer` is
///   "the base domain of the identity provider that issues JSON Web Tokens", so a JWT
///   authorizer cannot validate a token without one.
/// * `AuthorizerUri` — "For REQUEST authorizers, this must be a well-formed Lambda
///   function URI": the Lambda to invoke, so that type cannot authorize without it.
/// * `AuthorizerPayloadFormatVersion` — "Specifies the format of the payload sent to an
///   HTTP API Lambda authorizer. Required for HTTP API Lambda authorizers. Supported
///   values are 1.0 and 2.0."
///
/// `EnableSimpleResponses` is deliberately NOT cross-checked against the payload format
/// version: a simple-response authorizer under payload format 1.0 is a configuration the
/// service accepts and that fails when the authorizer is INVOKED, not when it is created.
///
/// Shared by every writer of an authorizer, so the requirement cannot differ between
/// the create and a later update.
pub fn validate_authorizer_config(
    protocol_type: &str,
    authorizer_type: &str,
    authorizer_uri: Option<&str>,
    payload_format_version: Option<&str>,
    jwt_configuration: Option<&wire::JWTConfiguration>,
) -> Result<(), String> {
    let is_websocket = protocol_type == WEBSOCKET_PROTOCOL_TYPE;
    if authorizer_type == AUTHORIZER_TYPE_JWT {
        if is_websocket {
            return Err(format!(
                "AuthorizerType {AUTHORIZER_TYPE_JWT} is supported only for HTTP APIs, \
                 and this API's ProtocolType is {protocol_type}"
            ));
        }
        let issuer = jwt_configuration.and_then(|configuration| configuration.issuer.as_deref());
        if issuer.is_none_or(str::is_empty) {
            return Err(format!(
                "JwtConfiguration with an Issuer is required for an authorizer of type \
                 {AUTHORIZER_TYPE_JWT}"
            ));
        }
    }
    if authorizer_type == AUTHORIZER_TYPE_REQUEST {
        if authorizer_uri.is_none_or(str::is_empty) {
            return Err(format!(
                "AuthorizerUri is required for an authorizer of type {AUTHORIZER_TYPE_REQUEST}"
            ));
        }
        if !is_websocket && payload_format_version.is_none_or(str::is_empty) {
            return Err(format!(
                "AuthorizerPayloadFormatVersion is required for an authorizer of type \
                 {AUTHORIZER_TYPE_REQUEST} on an API whose ProtocolType is {protocol_type}"
            ));
        }
    }
    if let Some(version) = payload_format_version {
        if !AUTHORIZER_PAYLOAD_FORMAT_VERSIONS.contains(&version) {
            return Err(format!(
                "AuthorizerPayloadFormatVersion must be one of {}, got {version}",
                AUTHORIZER_PAYLOAD_FORMAT_VERSIONS.join(", ")
            ));
        }
    }
    Ok(())
}

/// `Ok(())`, or why this route's authorization configuration cannot hold together —
/// the message for the operation's `BadRequestException`.
///
/// One rule: a route whose `AuthorizationType` hands the decision to an `Authorizer`
/// resource ([`AUTHORIZER_BACKED_AUTHORIZATION_TYPES`]) must be associated with one,
/// because `Route$AuthorizerId` is what names it and nothing else on the route does.
///
/// Shared by every writer of a route, so the requirement cannot differ between the
/// create and a later update; an update validates the MERGED record, so a route that
/// already names an authorizer may switch type without resending the id.
pub fn validate_route_authorization(
    authorization_type: &str,
    authorizer_id: Option<&str>,
) -> Result<(), String> {
    if AUTHORIZER_BACKED_AUTHORIZATION_TYPES.contains(&authorization_type)
        && authorizer_id.is_none_or(str::is_empty)
    {
        return Err(format!(
            "AuthorizerId is required for a route whose AuthorizationType is \
             {authorization_type}"
        ));
    }
    Ok(())
}

/// One request member the model restricts to APIs of a single `ProtocolType`.
///
/// API Gateway v2 serves two protocols from one control plane, and a member's own
/// documentation is what says which of them supports it ("Supported only for WebSocket
/// APIs", "Supported only for HTTP APIs"). A request that configures such a member on an
/// api of the OTHER protocol asks for a resource that protocol has no behaviour for, so
/// the service refuses it rather than storing a setting nothing could ever apply.
#[derive(Debug, Clone, Copy)]
pub struct ProtocolRestrictedMember<'a> {
    /// The member's name as a request spells it, for the refusal message.
    pub member: &'a str,
    /// The one `ProtocolType` whose APIs support the member, from
    /// `PROTOCOL_TYPE_VALUES`.
    pub supported_by: &'a str,
    /// Does THIS request configure the member? A member the request left out asks for
    /// nothing, and neither does one set to the value it would carry anyway — an empty
    /// map or list, or a `false` flag whose documented default is `false` — so a caller
    /// may send those to an api of either protocol.
    pub configured: bool,
}

impl<'a> ProtocolRestrictedMember<'a> {
    /// A member whose documentation reads "Supported only for WebSocket APIs".
    pub fn websocket_only(member: &'a str, configured: bool) -> Self {
        Self {
            member,
            supported_by: WEBSOCKET_PROTOCOL_TYPE,
            configured,
        }
    }

    /// A member whose documentation reads "Supported only for HTTP APIs".
    pub fn http_only(member: &'a str, configured: bool) -> Self {
        Self {
            member,
            supported_by: HTTP_PROTOCOL_TYPE,
            configured,
        }
    }
}

/// Refuse a request that configures a [`ProtocolRestrictedMember`] on an api of another
/// protocol.
///
/// Every writer of a protocol-restricted member routes through here, so the gate cannot
/// differ between the operations that share a member, and an api of the supporting
/// protocol is never refused. The FIRST offending member is the one reported — a request
/// fixing it is checked again.
///
/// `Ok(())` or the message for the operation's `BadRequestException`, built from the
/// request in the sentence API Gateway answers a protocol-restricted request with
/// ("Currently, Models are not permitted for APIs with a protocol type of HTTP"): the
/// member the request carried, and the protocol type of the api it was aimed at.
pub fn validate_protocol_restricted_members(
    protocol_type: &str,
    members: &[ProtocolRestrictedMember<'_>],
) -> Result<(), String> {
    for restricted in members {
        if restricted.configured && protocol_type != restricted.supported_by {
            return Err(format!(
                "Currently, {} is not permitted for APIs with a protocol type of \
                 {protocol_type}",
                restricted.member
            ));
        }
    }
    Ok(())
}

/// Refuse a child resource one protocol's APIs have no behaviour for, on an api of
/// another protocol — the whole-resource form of
/// [`validate_protocol_restricted_members`], for a create whose every member is moot
/// once the parent's protocol is wrong.
///
/// `resource_plural` names the resource as the service's own messages do (`Models`,
/// `Route responses`), and the message is the sentence API Gateway answers such a
/// request with, built from the parent api's protocol type: "Currently, Models are not
/// permitted for APIs with a protocol type of HTTP". `Ok(())` or the message for the
/// operation's `BadRequestException`.
pub fn validate_protocol_restricted_resource(
    protocol_type: &str,
    resource_plural: &str,
    supported_by: &str,
) -> Result<(), String> {
    if protocol_type == supported_by {
        return Ok(());
    }
    Err(format!(
        "Currently, {resource_plural} are not permitted for APIs with a protocol type \
         of {protocol_type}"
    ))
}

/// Is `name` a `Model$Name` the service accepts?
///
/// "The name of the model. Must be alphanumeric" (`CreateModel$Name`), so a name
/// carrying anything but letters and digits is refused. The length bounds (1..128) are
/// the modelled constraint and are already refused before a handler runs, so this is
/// only the alphabet.
///
/// `CreateModel` and `UpdateModel` both route through here: the requirement is a
/// property of the request, so it cannot differ between the two.
pub fn model_name_is_alphanumeric(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|character| character.is_ascii_alphanumeric())
}

/// Does `content_type` name a media type whose schema language is JSON Schema?
///
/// A JSON media type is one whose subtype is `json` or ends in `+json`
/// (RFC 6839's `+json` structured syntax suffix), with any parameters
/// (`; charset=utf-8`) ignored. A model that names NO content type is a JSON model
/// too: `application/json` is the content type the service's own documentation gives
/// for a model (`Model$ContentType`: "for example, \"application/json\"").
pub fn is_json_media_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return true;
    };
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let subtype = media_type.split('/').nth(1).unwrap_or_default();
    subtype == "json" || subtype.ends_with("+json")
}

/// Refuse a `Schema` that is not a schema document of the model's content type.
///
/// For a JSON model (see [`is_json_media_type`]) the schema is a JSON Schema document:
/// "For application/json models, this should be JSON schema draft 4 model"
/// (`CreateModel$Schema`), and "A JSON Schema is a JSON document, and that document
/// MUST be an object" (JSON Schema draft-04, section 3.2). A schema that does not
/// parse as JSON, or parses as a value that is not an object, is therefore no JSON
/// Schema at all and nothing reading the model could treat it as one.
///
/// A model of any OTHER content type carries a schema in that content type's own
/// language, which this service has no reader for, so its schema is stored verbatim
/// rather than refused for not being JSON.
///
/// What this does NOT do is check a document against the whole of draft-04 — that
/// every keyword is one draft-04 defines and carries a value of the type draft-04
/// gives it. A partial schema is legitimate input (`{}` matches everything), and the
/// document is handed back verbatim by every reader of the model.
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the
/// member and what was wrong with it, and echoes no schema back.
pub fn validate_model_schema(content_type: Option<&str>, schema: &str) -> Result<(), String> {
    if !is_json_media_type(content_type) {
        return Ok(());
    }
    let document: ::serde_json::Value = ::serde_json::from_str(schema)
        .map_err(|error| format!("Invalid model specified: Schema is not valid JSON: {error}"))?;
    if document.is_object() {
        return Ok(());
    }
    // The JSON type the schema turned out to be, so the caller can see what it sent
    // without the document being echoed back.
    let kind = match document {
        ::serde_json::Value::Null => "null",
        ::serde_json::Value::Bool(_) => "a boolean",
        ::serde_json::Value::Number(_) => "a number",
        ::serde_json::Value::String(_) => "a string",
        ::serde_json::Value::Array(_) => "an array",
        ::serde_json::Value::Object(_) => "an object",
    };
    Err(format!(
        "Invalid model specified: Schema is not a valid JSON schema document: a JSON schema \
         is a JSON object, but this Schema is {kind}."
    ))
}

/// A persisted `Stage` resource, owned by its parent [`Api`].
///
/// The record keeps what the create resolved — the name it is keyed by, the instants
/// it was created and last changed at, and the settings maps the request supplied — so
/// every reader of the stage reports one value, and [`Stage::to_wire`] projects it onto
/// the modelled shape the handlers serialize.
#[derive(Debug, Clone)]
pub struct Stage {
    pub stage_name: String,
    pub description: Option<String>,
    pub auto_deploy: bool,
    pub tags: BTreeMap<String, String>,
    /// The `DeploymentId` this stage serves, set by the create that deployed to it:
    /// "A Deployment must be associated with a Stage for it to be callable over the
    /// internet" (the `Deployment` shape's own doc).
    pub deployment_id: Option<String>,
    pub access_log_settings: Option<wire::AccessLogSettings>,
    /// "Supported only for WebSocket APIs" (`Stage$ClientCertificateId`).
    pub client_certificate_id: Option<String>,
    pub default_route_settings: Option<wire::RouteSettings>,
    /// Route settings by `routeKey`. `BTreeMap`, so the stage reports them in one
    /// stable order however the request ordered them.
    pub route_settings: BTreeMap<String, wire::RouteSettings>,
    /// The stage variable map, by variable name.
    pub stage_variables: BTreeMap<String, String>,
    /// "Describes the status of the last deployment of a stage. Supported only for
    /// stages with autoDeploy enabled" (`Stage$LastDeploymentStatusMessage`), so a
    /// stage that never auto-released one has none.
    pub last_deployment_status_message: Option<String>,
    pub created_date: SystemTime,
    /// "The timestamp when the stage was last updated" (`Stage$LastUpdatedDate`): the
    /// creation instant until something rewrites the record.
    pub last_updated_date: SystemTime,
    /// True only for a stage another operation created on the caller's behalf: "If you
    /// created an API using quick create, the $default stage is managed by API Gateway"
    /// (`Stage$ApiGatewayManaged`). A stage the caller asked for is its own.
    pub api_gateway_managed: bool,
}

impl Stage {
    /// Project the record onto the modelled `Stage` shape.
    pub fn to_wire(&self) -> wire::Stage {
        wire::Stage {
            access_log_settings: self.access_log_settings.clone(),
            api_gateway_managed: Some(self.api_gateway_managed),
            auto_deploy: Some(self.auto_deploy),
            client_certificate_id: self.client_certificate_id.clone(),
            created_date: Some(to_timestamp(self.created_date)),
            default_route_settings: self.default_route_settings.clone(),
            deployment_id: self.deployment_id.clone(),
            description: self.description.clone(),
            last_deployment_status_message: self.last_deployment_status_message.clone(),
            last_updated_date: Some(to_timestamp(self.last_updated_date)),
            route_settings: Some(
                self.route_settings
                    .iter()
                    .map(|(key, settings)| (key.clone(), settings.clone()))
                    .collect(),
            ),
            stage_name: self.stage_name.clone(),
            stage_variables: Some(
                self.stage_variables
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect(),
            ),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        }
    }
}

/// The most stages one api may hold, after which CreateStage is refused. AWS's
/// published API Gateway quota is 10 stages per API.
pub const MAX_STAGES_PER_API: usize = 10;

/// The `StageName` of the stage that serves an api's root: "the $default stage"
/// (`Stage$ApiGatewayManaged`), the one name that is not a plain stage name.
pub const DEFAULT_STAGE_NAME: &str = "$default";

/// The longest `StageName` the service accepts: the upper bound of
/// `StringWithLengthBetween1And128`, the shape `CreateStage$StageName` targets.
pub const MAX_STAGE_NAME_LEN: usize = 128;

/// The longest stage variable VALUE the service accepts: the upper bound of
/// `StringWithLengthBetween0And2048`, the value shape of `StageVariablesMap`
/// (`smithy_query(query="shape", shape="StageVariablesMap")`).
pub const MAX_STAGE_VARIABLE_VALUE_LEN: usize = 2048;

/// Refuse a `StageName` the service does not accept.
///
/// "Stage names can only contain alphanumeric characters, hyphens, and underscores.
/// Maximum length is 128 characters" (the `CreateStage$StageName` API reference), plus
/// [`DEFAULT_STAGE_NAME`], the reserved name of the stage that serves an api's root.
/// The length bound of `StringWithLengthBetween1And128` carries no `@length` trait in
/// the model, so the request walk does not enforce it and this does.
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the
/// member and the rule it broke.
pub fn validate_stage_name(stage_name: &str) -> Result<(), String> {
    if stage_name == DEFAULT_STAGE_NAME {
        return Ok(());
    }
    let length = stage_name.chars().count();
    if length == 0 || length > MAX_STAGE_NAME_LEN {
        return Err(format!(
            "Invalid value for StageName: must have length between 1 and \
             {MAX_STAGE_NAME_LEN}, got {length}."
        ));
    }
    if !stage_name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
    {
        return Err(format!(
            "Invalid value for StageName: stage names can only contain alphanumeric \
             characters, hyphens and underscores, or be the reserved name \
             {DEFAULT_STAGE_NAME}, got '{stage_name}'."
        ));
    }
    Ok(())
}

/// Refuse a stage variable whose name or value is not one the service accepts.
///
/// "Variable names can have alphanumeric and underscore characters, and the values
/// must match [A-Za-z0-9-._~:/?#&=,]+" (`Stage$StageVariables`), and a value is bounded
/// by [`MAX_STAGE_VARIABLE_VALUE_LEN`], the upper bound of the map's value shape.
/// Neither is a modelled trait, so the request walk does not enforce them and this
/// does.
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the
/// variable and the rule it broke.
pub fn validate_stage_variables<'a, I>(variables: I) -> Result<(), String>
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    for (name, value) in variables {
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(format!(
                "Invalid value for StageVariables: variable names can have alphanumeric \
                 and underscore characters, got '{name}'."
            ));
        }
        let length = value.chars().count();
        if length > MAX_STAGE_VARIABLE_VALUE_LEN {
            return Err(format!(
                "Invalid value for StageVariables: the value of '{name}' must have length \
                 less than or equal to {MAX_STAGE_VARIABLE_VALUE_LEN}, got {length}."
            ));
        }
        if value.is_empty() || !value.chars().all(is_stage_variable_value_character) {
            return Err(format!(
                "Invalid value for StageVariables: the value of '{name}' must match \
                 [A-Za-z0-9-._~:/?#&=,]+."
            ));
        }
    }
    Ok(())
}

/// Is `character` one the stage variable value pattern `[A-Za-z0-9-._~:/?#&=,]`
/// admits (`Stage$StageVariables`)?
fn is_stage_variable_value_character(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(
            character,
            '-' | '.' | '_' | '~' | ':' | '/' | '?' | '#' | '&' | '=' | ','
        )
}

/// The `$context` variable an access log format must select, so an access log entry can
/// be tied to the request that produced it: "The format must include at least
/// $context.requestId" (`AccessLogSettings$Format`).
pub const REQUIRED_ACCESS_LOG_CONTEXT_VARIABLE: &str = "$context.requestId";

/// Refuse `AccessLogSettings` whose `Format` could not identify the request it logged.
///
/// "The format must include at least $context.requestId"
/// (`AccessLogSettings$Format`), so a format selecting no request id is one no access
/// log entry could be traced back to a request with. A settings block that names no
/// format at all configures no access logging, so there is no format to require
/// anything of.
///
/// Every operation that stores access log settings routes through here: the requirement
/// is a property of the settings, so it cannot differ between them.
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the
/// member and the rule it broke.
pub fn validate_access_log_settings(settings: &wire::AccessLogSettings) -> Result<(), String> {
    let Some(format) = settings.format.as_ref() else {
        return Ok(());
    };
    if format.contains(REQUIRED_ACCESS_LOG_CONTEXT_VARIABLE) {
        return Ok(());
    }
    Err(format!(
        "Invalid value for AccessLogSettings: the Format must include at least \
         {REQUIRED_ACCESS_LOG_CONTEXT_VARIABLE}, got '{format}'."
    ))
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

/// A persisted custom `DomainName` resource.
///
/// Keyed in the store by [`DomainName::domain_name`], which is the identifier every
/// operation on the resource addresses it by. `DomainNameStatus` is deliberately NOT a
/// field: it is derived from the clock on every read (see [`DomainName::status`]), so no
/// stored copy of it can go stale.
#[derive(Debug, Clone)]
pub struct DomainName {
    pub domain_name: String,
    /// The resource's ARN, built once at create time from the request's region so it
    /// stays stable for the life of the resource.
    pub domain_name_arn: String,
    /// The regional host API Gateway serves this domain name on, minted with the
    /// resource and stable across every update: it names the endpoint a caller points
    /// their own DNS record at, so changing a certificate or a routing mode cannot move
    /// it. See [`api_gateway_domain_host`].
    pub api_gateway_domain_name: String,
    /// This domain name's endpoint configurations, in the order the request gave them —
    /// `DomainNameConfigurations` is a modelled list, not a map, so order is the
    /// caller's and is preserved.
    pub configurations: Vec<DomainNameEndpoint>,
    /// The mutual-TLS truststore, held as the modelled INPUT shape: `TruststoreUri` and
    /// `TruststoreVersion` are the caller's, while `TruststoreWarnings` is the service's
    /// report on the truststore and is projected in [`DomainName::to_wire`].
    pub mutual_tls_authentication: Option<wire::MutualTlsAuthenticationInput>,
    /// One of `ROUTING_MODE_VALUES`.
    pub routing_mode: String,
    pub tags: BTreeMap<String, String>,
    /// The instant the endpoint migration started by the last change to
    /// [`DomainName::configurations`] is due to finish, or `None` for a domain name
    /// whose endpoints have never been migrated. Until it passes the domain name reports
    /// `UPDATING` and refuses a further update; after it, `AVAILABLE`.
    pub migration_due_at: Option<SystemTime>,
}

impl DomainName {
    /// This domain name's `DomainNameStatus` as of `now`.
    ///
    /// `UPDATING` while the migration the last endpoint-configuration change started is
    /// still due, `AVAILABLE` otherwise — "If the status is UPDATING, the domain cannot
    /// be modified further until the existing operation is complete. If it is AVAILABLE,
    /// the domain can be updated" (`DomainNameConfiguration$DomainNameStatus`). Derived,
    /// not stored, so reading a domain name never has to write one.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match self.migration_due_at {
            Some(due) if now < due => DOMAIN_NAME_STATUS_UPDATING,
            _ => DOMAIN_NAME_STATUS_AVAILABLE,
        }
    }

    /// Project the record onto the modelled `DomainName` shape, as of `now`.
    pub fn to_wire(&self, now: SystemTime) -> wire::DomainName {
        let status = self.status(now);
        wire::DomainName {
            api_mapping_selection_expression: Some(
                API_MAPPING_SELECTION_EXPRESSION.to_string(),
            ),
            domain_name: self.domain_name.clone(),
            domain_name_arn: Some(self.domain_name_arn.clone()),
            domain_name_configurations: Some(
                self.configurations
                    .iter()
                    .map(|endpoint| endpoint.to_wire(&self.api_gateway_domain_name, status))
                    .collect(),
            ),
            mutual_tls_authentication: self.mutual_tls_authentication.as_ref().map(|input| {
                wire::MutualTlsAuthentication {
                    truststore_uri: input.truststore_uri.clone(),
                    truststore_version: input.truststore_version.clone(),
                    // "A list of warnings that API Gateway returns while processing your
                    // truststore. Invalid certificates produce warnings"
                    // (`MutualTlsAuthentication$TruststoreWarnings`). Nothing was
                    // fetched or parsed — the truststore is an S3 URL this emulator does
                    // not read — so there is no warning to report and the list is empty
                    // rather than invented.
                    truststore_warnings: Some(Vec::new()),
                }
            }),
            routing_mode: Some(self.routing_mode.clone()),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        }
    }
}

/// One entry of a domain name's `DomainNameConfigurations`.
///
/// Holds only what a request sets plus the members the service resolves ONCE at write
/// time (the three defaulted enums). `DomainNameStatus` and `ApiGatewayDomainName` are
/// not here: the first is derived from the clock and the second belongs to the domain
/// name as a whole, so both are supplied by [`DomainNameEndpoint::to_wire`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainNameEndpoint {
    pub certificate_arn: Option<String>,
    pub certificate_name: Option<String>,
    pub ownership_verification_certificate_arn: Option<String>,
    /// One of `ENDPOINT_TYPE_VALUES`.
    pub endpoint_type: String,
    /// One of `SECURITY_POLICY_VALUES`.
    pub security_policy: String,
    /// One of `IP_ADDRESS_TYPE_VALUES`.
    pub ip_address_type: String,
}

impl DomainNameEndpoint {
    /// Project the entry onto the modelled `DomainNameConfiguration` shape, carrying the
    /// owning domain name's host and its current status.
    pub fn to_wire(
        &self,
        api_gateway_domain_name: &str,
        domain_name_status: &str,
    ) -> wire::DomainNameConfiguration {
        wire::DomainNameConfiguration {
            api_gateway_domain_name: Some(api_gateway_domain_name.to_string()),
            certificate_arn: self.certificate_arn.clone(),
            certificate_name: self.certificate_name.clone(),
            // "The timestamp when the certificate … was uploaded"
            // (`DomainNameConfiguration$CertificateUploadDate`) is a fact about the
            // certificate in ACM, not about this resource: nothing was uploaded here and
            // no rule derives the instant from the request, so it is left unset rather
            // than filled with an instant that would claim an upload that never happened.
            certificate_upload_date: None,
            domain_name_status: Some(domain_name_status.to_string()),
            // Reported only alongside a status that needs explaining; `AVAILABLE` and
            // the migration this service runs carry no detail beyond the status itself.
            domain_name_status_message: None,
            endpoint_type: Some(self.endpoint_type.clone()),
            // "The Amazon Route 53 Hosted Zone ID of the endpoint"
            // (`DomainNameConfiguration$HostedZoneId`) names a zone in Route 53 that
            // this emulator neither owns nor can look up, and no rule derives it from the
            // request. Left unset: a made-up zone id is one a caller could put in a real
            // DNS alias record.
            hosted_zone_id: None,
            ip_address_type: Some(self.ip_address_type.clone()),
            security_policy: Some(self.security_policy.clone()),
            ownership_verification_certificate_arn: self
                .ownership_verification_certificate_arn
                .clone(),
        }
    }
}

/// The ARN of a `DomainName` resource in `region`.
///
/// API Gateway's resource portion for a custom domain name is `/domainnames/{name}`, and
/// the account segment is the emulator's canonical one (see `engine::emulator_arn`).
pub fn domain_name_arn(domain_name: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("/domainnames/{domain_name}"))
}

/// The regional host API Gateway serves a custom domain name on: the
/// `d-{id}.execute-api.{region}.amazonaws.com` form a caller aliases their own DNS
/// record to, built from an id this store minted (see
/// [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_domain_name_id`]) and the api
/// endpoint host `CreateApiResponse$ApiEndpoint` documents.
pub fn api_gateway_domain_host(domain_id: &str, region: &str) -> String {
    format!("d-{domain_id}.execute-api.{region}.amazonaws.com")
}

/// Resolve one `DomainNameConfigurations` entry of a request into the record to store.
///
/// The three enum members are defaulted here and nowhere else, so a create and an update
/// resolve them identically. `Err` carries the message for the operation's
/// `BadRequestException`: `DomainNameStatus` is service-computed, so a request that sets
/// it is naming a value it does not own, and each enum is checked against its modelled
/// catalog so an unexpected value is named in the refusal rather than stored.
pub fn resolve_domain_name_endpoint(
    index: usize,
    requested: &wire::DomainNameConfiguration,
) -> Result<DomainNameEndpoint, String> {
    if let Some(status) = requested.domain_name_status.as_deref() {
        return Err(format!(
            "DomainNameConfigurations[{index}].DomainNameStatus is set by the service and \
             cannot be specified in a request; got '{status}'."
        ));
    }
    let endpoint_type = checked_enum(
        index,
        "EndpointType",
        requested.endpoint_type.as_deref(),
        DEFAULT_ENDPOINT_TYPE,
        wire::ENDPOINT_TYPE_VALUES,
    )?;
    let security_policy = checked_enum(
        index,
        "SecurityPolicy",
        requested.security_policy.as_deref(),
        DEFAULT_SECURITY_POLICY,
        wire::SECURITY_POLICY_VALUES,
    )?;
    let ip_address_type = checked_enum(
        index,
        "IpAddressType",
        requested.ip_address_type.as_deref(),
        DEFAULT_IP_ADDRESS_TYPE,
        wire::IP_ADDRESS_TYPE_VALUES,
    )?;
    Ok(DomainNameEndpoint {
        certificate_arn: requested.certificate_arn.clone(),
        certificate_name: requested.certificate_name.clone(),
        ownership_verification_certificate_arn: requested
            .ownership_verification_certificate_arn
            .clone(),
        endpoint_type,
        security_policy,
        ip_address_type,
    })
}

/// Resolve a request's `RoutingMode` against the modelled catalog.
///
/// `Ok(None)` means the request named none, leaving the defaulting decision with the
/// caller — a create applies [`DEFAULT_ROUTING_MODE`], an update leaves the stored value
/// alone — while the catalog check and its wording live here, so the two handlers cannot
/// drift. `Err` carries the message for the operation's `BadRequestException`.
///
/// `RoutingMode` is a modelled enum, so the boundary validator already refuses a value
/// outside the catalog; checking it here too means an unexpected value is named in the
/// refusal rather than stored and reported back.
pub fn resolve_routing_mode(requested: Option<&str>) -> Result<Option<String>, String> {
    match requested {
        None => Ok(None),
        Some(mode) if wire::ROUTING_MODE_VALUES.contains(&mode) => Ok(Some(mode.to_string())),
        Some(mode) => Err(format!(
            "Invalid value for RoutingMode: '{mode}'. Valid values are: {}.",
            wire::ROUTING_MODE_VALUES.join(", ")
        )),
    }
}

/// One enum member of a `DomainNameConfigurations` entry: the requested value when it is
/// in `catalog`, `default` when the request names none.
fn checked_enum(
    index: usize,
    member: &str,
    requested: Option<&str>,
    default: &str,
    catalog: &[&str],
) -> Result<String, String> {
    match requested {
        None => Ok(default.to_string()),
        Some(value) if catalog.contains(&value) => Ok(value.to_string()),
        Some(value) => Err(format!(
            "Invalid value for DomainNameConfigurations[{index}].{member}: '{value}'. \
             Valid values are: {}.",
            catalog.join(", ")
        )),
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

/// The URI scheme of an api's default endpoint, chosen by its `ProtocolType`.
///
/// `ApiEndpoint` is the value a caller hands to a client library to reach the api, so
/// its scheme has to name the protocol that api speaks: a WebSocket API is opened with
/// an RFC 6455 handshake, whose URI scheme is `wss` (secure) — `ws` is the cleartext
/// form, and nothing this emulator serves is cleartext — while an HTTP API is invoked
/// over `https` ("clients can invoke your API with the default
/// https://{api_id}.execute-api.{region}.amazonaws.com endpoint",
/// `Api$DisableExecuteApiEndpoint`).
///
/// `ProtocolType` is an enum of exactly `HTTP | WEBSOCKET`
/// (`smithy_query(query="shape", shape="ProtocolType")`) and the boundary validator
/// refuses anything else, so `WEBSOCKET` and the `HTTP` arm cover every api a record
/// can hold; that arm doubles as the fallback a future enum value would take, which
/// gives it the scheme an api is invoked over rather than one no client could open.
pub fn execute_api_scheme(protocol_type: &str) -> &'static str {
    match protocol_type {
        WEBSOCKET_PROTOCOL_TYPE => "wss",
        // HTTP_PROTOCOL_TYPE, and anything the enum gains later.
        _ => "https",
    }
}

/// The default execute-api endpoint of an api: the URI clients reach it on when
/// no custom domain name is configured.
///
/// The host is the one `CreateApiResponse$ApiEndpoint` documents —
/// `{api-id}.execute-api.{region}.amazonaws.com` — and the scheme comes from the
/// api's `ProtocolType` (see [`execute_api_scheme`]).
pub fn execute_api_endpoint(api_id: &str, region: &str, protocol_type: &str) -> String {
    format!(
        "{}://{api_id}.execute-api.{region}.amazonaws.com",
        execute_api_scheme(protocol_type)
    )
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
