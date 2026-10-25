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
use std::time::{Duration, Instant, SystemTime};

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

/// The prefix a `Route$Target` carries when the route is wired to an integration of
/// its own api: API Gateway addresses an integration target as
/// `integrations/{IntegrationId}` — the form `CreateRoute$Target` takes and the one
/// every route reader reports back. [`Route::target_integration_id`] is what reads an
/// `IntegrationId` out of a target, so no handler has to know the spelling.
pub const INTEGRATION_TARGET_PREFIX: &str = "integrations/";

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

/// The most integration responses one integration may hold, after which
/// CreateIntegrationResponse is refused.
///
/// AWS publishes no separate integration-responses-per-integration quota, so this is a
/// CHOSEN bound, set to the per-parent quota this service already enforces for an api's
/// integrations ([`MAX_INTEGRATIONS_PER_API`]): what it buys is that no loop of
/// CreateIntegrationResponse calls can grow this store without end, and that the
/// per-integration duplicate-key scan stays bounded. `TooManyRequestsException` ("A
/// limit has been exceeded") is the modelled error the operation refuses with.
pub const MAX_INTEGRATION_RESPONSES_PER_INTEGRATION: usize = 300;

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

/// The refusal a request carries when its `DomainNameConfigurations` names no endpoint:
/// the message of the `BadRequestException` both domain-name writers answer with (see
/// [`resolve_domain_name_endpoints`]). It names the member's constraint — one
/// configuration is expected — and quotes nothing from the request.
pub const EMPTY_DOMAIN_NAME_CONFIGURATIONS: &str =
    "Invalid input. Expected one domain name configuration";

/// The most authorizers one api may hold, after which CreateAuthorizer is refused.
///
/// API Gateway publishes a quota of 10 authorizers per API, so this is the faithful
/// bound AND what stops a loop of CreateAuthorizer calls growing this store without
/// end — every call mints a new authorizer, so nothing else bounds it.
pub const MAX_AUTHORIZERS_PER_API: usize = 10;

/// The most `IdentitySource` entries one authorizer may carry, after which
/// CreateAuthorizer is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: `IdentitySourceList` carries no
/// modelled length constraint, so nothing otherwise bounds the list one request stores —
/// and every request a WebSocket api's authorization step serves then reads the whole of
/// it (`crate::service`'s connection-request authorization), which is what makes an
/// unbounded list per-request work rather than only stored bytes. The figure is set far
/// above the one to three identity sources a real authorizer names, so what it refuses is
/// an absurd list rather than a configuration a caller would write. Do not cite it as a
/// published quota.
pub const MAX_IDENTITY_SOURCES_PER_AUTHORIZER: usize = 100;

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

/// The `IntegrationType` of a proxy integration to a Lambda function or another AWS
/// service action, from `INTEGRATION_TYPE_VALUES`: "AWS_PROXY: for integrating the route
/// or method request with a Lambda function or other AWS service action. This
/// integration is also referred to as a Lambda proxy integration"
/// (`CreateIntegration$IntegrationType`).
pub const AWS_PROXY_INTEGRATION_TYPE: &str = "AWS_PROXY";

/// The `IntegrationType` of a proxy integration to an HTTP endpoint, from
/// `INTEGRATION_TYPE_VALUES` ("HTTP_PROXY: for integrating the route or method request
/// with an HTTP endpoint … also referred to as the HTTP proxy integration").
pub const HTTP_PROXY_INTEGRATION_TYPE: &str = "HTTP_PROXY";

/// The one `PayloadFormatVersion` every integration supports: "For all other
/// integrations, 1.0 is the only supported value"
/// (`Integration$PayloadFormatVersion`).
pub const ONLY_PAYLOAD_FORMAT_VERSION: &str = "1.0";

/// The later `PayloadFormatVersion` a Lambda proxy integration supports: "Supported
/// values for Lambda proxy integrations are 1.0 and 2.0"
/// (`Integration$PayloadFormatVersion`).
pub const LAMBDA_PROXY_PAYLOAD_FORMAT_VERSION: &str = "2.0";

/// Every `PayloadFormatVersion` a Lambda proxy integration supports, from the same
/// sentence ("Supported values for Lambda proxy integrations are 1.0 and 2.0").
pub const LAMBDA_PROXY_PAYLOAD_FORMAT_VERSIONS: &[&str] =
    &[ONLY_PAYLOAD_FORMAT_VERSION, LAMBDA_PROXY_PAYLOAD_FORMAT_VERSION];

/// The `TimeoutInMillis` an integration of an HTTP API gets when the request names
/// none: "The default timeout is … 30 seconds for HTTP APIs"
/// (`CreateIntegration$TimeoutInMillis`).
pub const DEFAULT_HTTP_TIMEOUT_MILLIS: i32 = 30_000;

/// The `TimeoutInMillis` window an integration's timeout has to fall in, from the shape
/// `CreateIntegration$TimeoutInMillis` targets: `IntegerWithLengthBetween50And30000`,
/// `range` 50..30000 (`smithy_query(query="shape",
/// shape="IntegerWithLengthBetween50And30000")`). The boundary validator is what refuses
/// a value outside it; these name the window the refusal states.
pub const MIN_INTEGRATION_TIMEOUT_MILLIS: i32 = 50;

/// The upper end of that window (see [`MIN_INTEGRATION_TIMEOUT_MILLIS`]).
pub const MAX_INTEGRATION_TIMEOUT_MILLIS: i32 = 30_000;

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

/// The most VPC links one account may hold, after which CreateVpcLink is refused.
///
/// API Gateway's published quota for the HTTP/WebSocket (ApiGatewayV2) API is 10 VPC
/// links per account per Region, so this is the faithful bound AND what stops a loop of
/// CreateVpcLink calls growing this store without end — every call mints a new link, so
/// nothing else bounds it.
pub const MAX_VPC_LINKS: usize = 10;

/// The `VpcLinkStatus` a VPC link carries while the service is still provisioning it,
/// from `VPC_LINK_STATUS_VALUES` (`AVAILABLE | DELETING | FAILED | INACTIVE |
/// PENDING`).
pub const VPC_LINK_STATUS_PENDING: &str = "PENDING";

/// The `VpcLinkStatus` of a VPC link that finished provisioning, from
/// `VPC_LINK_STATUS_VALUES`.
pub const VPC_LINK_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The only `VpcLinkVersion` this API models (`VPC_LINK_VERSION_VALUES`: `V2`), so it is
/// the version every link this service mints reports.
pub const VPC_LINK_VERSION_V2: &str = "V2";

/// How long a created VPC link stays [`VPC_LINK_STATUS_PENDING`] before it reports
/// [`VPC_LINK_STATUS_AVAILABLE`]: API Gateway attaches the link's network interfaces to
/// the subnets it was given, which is not done when the create call answers.
///
/// API Gateway documents no duration for it ("it can take up to 10 minutes" is
/// guidance, not a contract), so this span is the emulator's own, chosen short enough
/// not to slow a caller down and long enough that a caller reading straight after
/// `CreateVpcLink` still observes [`VPC_LINK_STATUS_PENDING`].
pub const VPC_LINK_CREATE_SPAN: Duration = Duration::from_millis(500);

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
    /// Every persisted portal, keyed by `PortalId`. A portal is an account-level
    /// resource, not a child of an api, so it lives beside [`Self::apis`] rather than
    /// inside one; `BTreeMap` for the same stable key order `ListPortals` pages over.
    pub portals: BTreeMap<String, Portal>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_portal_id`]; see
    /// [`ID_MULTIPLIER`] for how an id is derived from it.
    pub portal_id_seed: u64,
    /// Every persisted portal product, keyed by `PortalProductId`. A portal product is
    /// an ACCOUNT-level resource, not a child of an api, so it is stored here beside
    /// the apis; `BTreeMap` for the same stable key order
    /// [`ComAmazonawsApigatewayv2Apigatewayv2State::apis`] is kept in, which is what
    /// makes `ListPortalProducts` paging resumable.
    pub portal_products: BTreeMap<String, PortalProduct>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_portal_product_id`].
    pub portal_product_id_seed: u64,
    /// Every persisted VPC link, keyed by `VpcLinkId`. A VPC link is an
    /// account-and-Region resource rather than a child of an api — an integration only
    /// REFERENCES one through its `ConnectionId` — so it is stored beside [`Self::apis`]
    /// and not inside one. `BTreeMap` for the same stable key order [`Self::apis`] has.
    pub vpc_links: BTreeMap<String, VpcLink>,
    /// Drives [`ComAmazonawsApigatewayv2Apigatewayv2State::mint_vpc_link_id`]; see
    /// [`ID_MULTIPLIER`] for how an id is derived from it.
    pub vpc_link_id_seed: u64,
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

    /// Mint a fresh `PortalId`: ten lowercase alphanumerics, never one already minted.
    ///
    /// `PortalId` is modelled as `__stringMin10Max30PatternAZ09` (len 10..30, pattern
    /// `^[a-z0-9]+$`), and [`mint_id`] renders exactly ten characters of
    /// [`ID_ALPHABET`], so every minted id satisfies that constraint by construction.
    pub fn mint_portal_id(&mut self) -> String {
        self.portal_id_seed = self.portal_id_seed.wrapping_add(1);
        mint_id(self.portal_id_seed)
    }

    /// Mint a fresh `PortalProductId`: ten lowercase alphanumerics, never one already
    /// minted. The modelled `__stringMin10Max30PatternAZ09` the member targets is
    /// `^[a-z0-9]+$`, 10..30 characters, which [`mint_id`]'s ten-character lowercase
    /// alphanumeric id satisfies.
    pub fn mint_portal_product_id(&mut self) -> String {
        self.portal_product_id_seed = self.portal_product_id_seed.wrapping_add(1);
        mint_id(self.portal_product_id_seed)
    }

    /// Mint a fresh `VpcLinkId`: ten lowercase alphanumerics, never one already minted.
    pub fn mint_vpc_link_id(&mut self) -> String {
        self.vpc_link_id_seed = self.vpc_link_id_seed.wrapping_add(1);
        mint_id(self.vpc_link_id_seed)
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

    /// Release a deployment of this api's current configuration BY THE SERVICE, for a
    /// stage whose `AutoDeploy` is on.
    ///
    /// Such a stage has its deployments released by the service rather than by a caller
    /// ("Specifies whether a deployment was automatically released",
    /// [`Deployment::auto_deployed`]), so the api's routes and integrations — which this
    /// record already holds — are released to it at `now`, the same instant the call
    /// that asked for the stage is resolving at. Both the stage quick create mints
    /// alongside an api and a `CreateStage` that asks for auto-deploy go through here,
    /// so one api cannot release two differently shaped deployments.
    ///
    /// Answers `(DeploymentId, LastDeploymentStatusMessage)`: the id of the deployment
    /// released, with the message the stage reports for it, or no id at all when the api
    /// is already at [`MAX_DEPLOYMENTS_PER_API`] — the quota belongs to the deployment,
    /// not to the stage, so the stage still exists and reports why its last deployment
    /// did not happen.
    pub fn release_auto_deployment(
        &mut self,
        now: SystemTime,
    ) -> (Option<String>, Option<String>) {
        if self.deployments.len() >= MAX_DEPLOYMENTS_PER_API {
            return (
                None,
                Some(format!(
                    "Failed to deploy stage: the maximum number of deployments \
                     ({MAX_DEPLOYMENTS_PER_API}) for this API has been reached."
                )),
            );
        }
        let released = self.mint_deployment_id();
        self.deployments.insert(
            released.clone(),
            Deployment {
                deployment_id: released.clone(),
                // The service released this one; no caller described it.
                description: None,
                deployment_status: DEPLOYMENT_STATUS_DEPLOYED.to_string(),
                deployment_status_message: None,
                auto_deployed: true,
                created_date: now,
            },
        );
        let message = format!("Successfully deployed stage with deployment id '{released}'");
        (Some(released), Some(message))
    }
}

impl Api {
    /// The ids of this api's routes that are wired to `integration_id`, in the store's
    /// stable `RouteId` order.
    ///
    /// A route holds the reference (through its `Target`), so the integration is the
    /// one that cannot be removed while any of them exists: a deleted integration
    /// would leave those routes pointing at nothing.
    pub fn routes_targeting_integration(&self, integration_id: &str) -> Vec<String> {
        self.routes
            .values()
            .filter(|route| route.target_integration_id() == Some(integration_id))
            .map(|route| route.route_id.clone())
            .collect()
    }

    /// The route KEYS of this api's routes that are associated with `authorizer_id`, in
    /// the store's stable `RouteId` order.
    ///
    /// A route holds the reference (through its `AuthorizerId`), so the authorizer is the
    /// one that cannot be removed while any of them exists: a deleted authorizer would
    /// leave those routes naming nothing. An empty `AuthorizerId` names no authorizer, the
    /// same reading [`Api::unresolved_route_reference`] gives it.
    ///
    /// The KEY is what identifies a route to a caller — it is the route's name within its
    /// api — so it is what a refusal about the reference names.
    pub fn route_keys_using_authorizer(&self, authorizer_id: &str) -> Vec<String> {
        self.routes
            .values()
            .filter(|route| {
                route
                    .authorizer_id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .is_some_and(|id| id == authorizer_id)
            })
            .map(|route| route.route_key.clone())
            .collect()
    }

    /// The refusal message for a reference of this route that this api cannot resolve,
    /// or `None` when every reference names a child resource this api holds.
    ///
    /// A route references two resources of its OWN api: the integration its `Target`
    /// addresses (`integrations/{IntegrationId}`) and the authorizer its `AuthorizerId`
    /// names. Neither may be stored unresolved — a route wired to nothing could never be
    /// invoked, and the api it belongs to could not be reproduced on AWS — so every
    /// writer of a route resolves them here before persisting anything. The reference is
    /// part of the REQUEST, so a request naming one that does not resolve is "not valid,
    /// for example, the input is incomplete or incorrect": the message is the
    /// operation's `BadRequestException`'s.
    pub fn unresolved_route_reference(
        &self,
        target: Option<&str>,
        authorizer_id: Option<&str>,
    ) -> Option<String> {
        if let Some(integration_id) = target.and_then(target_integration_id) {
            if !self.integrations.contains_key(integration_id) {
                // The message names the kind of target the key addresses and the id it
                // carried, both read out of the request's own `Target`.
                return Some(format!(
                    "Unable to find route target of type {} with ID {integration_id}",
                    INTEGRATION_TARGET_PREFIX.trim_end_matches('/')
                ));
            }
        }
        if let Some(authorizer_id) = authorizer_id.filter(|id| !id.is_empty()) {
            if !self.authorizers.contains_key(authorizer_id) {
                return Some(UNRESOLVED_AUTHORIZER_MESSAGE.to_string());
            }
        }
        None
    }

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
    /// The `IntegrationId` this route is wired to, when its target names an
    /// integration of the same api (`integrations/{IntegrationId}`). `None` for a
    /// route with no target at all, and for a target of any other form — a target
    /// this service does not resolve to an integration references none of them.
    pub fn target_integration_id(&self) -> Option<&str> {
        self.target.as_deref().and_then(target_integration_id)
    }

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
    /// This integration's integration responses, keyed by `IntegrationResponseId`.
    /// Owned by the integration, so deleting the integration (or its api) cascades to
    /// them in one `remove`.
    pub integration_responses: BTreeMap<String, IntegrationResponse>,
    /// Drives [`Integration::mint_integration_response_id`]. Per-integration, because
    /// an `IntegrationResponseId` only has to be unique within the integration that
    /// owns the response.
    pub integration_response_id_seed: u64,
}

impl Integration {
    /// Mint a fresh `IntegrationResponseId` for this integration: ten lowercase
    /// alphanumerics, never one this integration already minted.
    pub fn mint_integration_response_id(&mut self) -> String {
        self.integration_response_id_seed = self.integration_response_id_seed.wrapping_add(1);
        mint_id(self.integration_response_id_seed)
    }

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
            // "The integration response selection expression for the integration.
            // Supported only for WebSocket APIs"
            // (`Integration$IntegrationResponseSelectionExpression`): no request member
            // sets it, and an integration response of this service is selected by the
            // `IntegrationResponseKey` it was stored under, so the record holds no such
            // expression to report.
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

/// A persisted `IntegrationResponse` resource, owned by its parent [`Integration`].
///
/// An integration response is WebSocket-only: its
/// `IntegrationResponse$ContentHandlingStrategy` is "Supported only for WebSocket
/// APIs", and the selection keys it is matched against are the WebSocket API's
/// integration response selection expressions.
#[derive(Debug, Clone, Default)]
pub struct IntegrationResponse {
    pub integration_response_id: String,
    /// Required on the modelled `IntegrationResponse` shape, so the record always
    /// carries one. Unique within the owning integration.
    pub integration_response_key: String,
    /// One of `CONTENT_HANDLING_STRATEGY_VALUES` when the request named one.
    pub content_handling_strategy: Option<String>,
    pub response_parameters: BTreeMap<String, String>,
    pub response_templates: BTreeMap<String, String>,
    pub template_selection_expression: Option<String>,
}

impl IntegrationResponse {
    /// Project the record onto the modelled `IntegrationResponse` shape.
    pub fn to_wire(&self) -> wire::IntegrationResponse {
        wire::IntegrationResponse {
            content_handling_strategy: self.content_handling_strategy.clone(),
            integration_response_id: Some(self.integration_response_id.clone()),
            integration_response_key: self.integration_response_key.clone(),
            response_parameters: Some(
                self.response_parameters
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            response_templates: Some(
                self.response_templates
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            ),
            template_selection_expression: self.template_selection_expression.clone(),
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
        return Err(invalid_authorizer_reference_message(authorization_type));
    }
    Ok(())
}

/// The sentence this service refuses a route that does not name a valid authorizer
/// with, in the form for the `AuthorizationType` the request set.
///
/// The refusal is about the authorizer REFERENCE — a route of an authorizer-backed
/// type that names no authorizer at all is as unauthorizable as one naming an id no
/// authorizer carries — so the sentence states the requirement the type imposes and
/// echoes no id back. `JWT` is the one authorizer-backed `AuthorizationType` that names
/// an authorizer of its own kind (`Route$AuthorizationType`: "JWT for using JSON Web
/// Tokens", and `AUTHORIZER_TYPE_VALUES` carries `JWT`), so its sentence names that
/// kind; `CUSTOM` is "for using a Lambda authorizer", whose requirement is the one
/// [`UNRESOLVED_AUTHORIZER_MESSAGE`] states.
pub fn invalid_authorizer_reference_message(authorization_type: &str) -> String {
    if authorization_type == AUTHORIZER_TYPE_JWT {
        return format!(
            "Invalid authorizer ID specified. Setting the authorization type to \
             {AUTHORIZER_TYPE_JWT} requires a valid {AUTHORIZER_TYPE_JWT} authorizer."
        );
    }
    UNRESOLVED_AUTHORIZER_MESSAGE.to_string()
}

/// The entry a CORS list carries to stand for "every value": the wildcard.
pub const CORS_WILDCARD: &str = "*";

/// `Ok(())`, or why this CORS configuration cannot hold together — the message for the
/// operation's `BadRequestException`.
///
/// One rule: a configuration that allows CREDENTIALED cross-origin requests cannot
/// answer them with the wildcard. A browser rejects a credentialed response whose
/// `Access-Control-Allow-Origin` (or `-Methods`, or `-Headers`) is `*`, so the pair
/// describes a policy no client could ever use and the service refuses it instead of
/// storing a document that would never work.
///
/// Shared by every writer of an api's CORS document, so the requirement cannot differ
/// between the create and a later update; an update checks the MERGED configuration,
/// which is the one the api would carry.
pub fn validate_cors_configuration(cors: &wire::Cors) -> Result<(), String> {
    if cors.allow_credentials != Some(true) {
        return Ok(());
    }
    // The message names the response header the wildcard would go into, which is what
    // the browser rule it states is about: `access-control-allow-origin: *` cannot
    // answer a credentialed request, and neither can the method or header forms.
    for (header, values) in [
        ("allow-origin", cors.allow_origins.as_ref()),
        ("allow-methods", cors.allow_methods.as_ref()),
        ("allow-headers", cors.allow_headers.as_ref()),
    ] {
        if values.is_some_and(|values| values.iter().any(|value| value == CORS_WILDCARD)) {
            return Err(format!(
                "allow-credentials is not supported if '{header}' is {CORS_WILDCARD}"
            ));
        }
    }
    Ok(())
}

/// The variable namespaces a selection expression may read, lowercased for comparison.
///
/// A selection expression selects a stored key by evaluating request DATA — the
/// documented defaults name exactly these namespaces (`$request.header.x-api-key` for
/// `Api$ApiKeySelectionExpression`, `$request.method $request.path` for an HTTP api's
/// `RouteSelectionExpression`, `$context.*` for context variables) — so a `$`-token
/// naming anything else names no variable at all.
pub const SELECTION_EXPRESSION_VARIABLES: &[&str] = &["request", "context", "stagevariables"];

/// `Ok(())`, or why this selection expression cannot be stored — the message for the
/// operation's `BadRequestException`.
///
/// One rule: in a selection expression an unescaped `$` begins a VARIABLE reference, so
/// every `$`-token has to name one of the variable namespaces the service evaluates
/// ([`SELECTION_EXPRESSION_VARIABLES`]). A selection KEY that itself begins with `$` —
/// `$default` is the one the service mints — is a literal, and a literal is written
/// escaped (`\$default`); unescaped it reads as a variable the service does not define.
pub fn validate_selection_expression(expression: &str) -> Result<(), String> {
    let characters: Vec<char> = expression.chars().collect();
    let mut index = 0;
    while index < characters.len() {
        match characters[index] {
            // An escaped character is a literal: this is how a selection key beginning
            // with `$` is written.
            '\\' => index += 2,
            '$' => {
                let name: String = characters[index + 1..]
                    .iter()
                    .skip_while(|c| **c == '{')
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                    .collect();
                if !SELECTION_EXPRESSION_VARIABLES.contains(&name.to_ascii_lowercase().as_str()) {
                    return Err(format!(
                        "Unexpected variable in selection expression: ${name}"
                    ));
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    Ok(())
}

/// `Ok(())`, or why this integration's payload format version cannot be stored — the
/// message for the operation's `BadRequestException`.
///
/// One rule, straight from `Integration$PayloadFormatVersion`: "Supported values for
/// Lambda proxy integrations are 1.0 and 2.0. For all other integrations, 1.0 is the
/// only supported value." A Lambda proxy integration is the `AWS_PROXY` type invoking a
/// function, so an `AWS_PROXY` integration that names an AWS service action through an
/// `IntegrationSubtype` instead is one of the "all other integrations".
pub fn validate_payload_format_version(
    integration_type: &str,
    integration_subtype: Option<&str>,
    payload_format_version: Option<&str>,
) -> Result<(), String> {
    let Some(version) = payload_format_version else {
        return Ok(());
    };
    let is_lambda_proxy =
        integration_type == AWS_PROXY_INTEGRATION_TYPE && integration_subtype.is_none();
    let supported = if is_lambda_proxy {
        LAMBDA_PROXY_PAYLOAD_FORMAT_VERSIONS
    } else {
        &[ONLY_PAYLOAD_FORMAT_VERSION]
    };
    if !supported.contains(&version) {
        // The refusal names the pair that cannot hold together — the version the request
        // sent and the type it sent it for — and nothing else: the cross-field rule is
        // about those two members.
        return Err(format!(
            "PayloadFormatVersion {version} is not supported for integration of type \
             {integration_type}"
        ));
    }
    Ok(())
}

/// The one `IntegrationType` that names no endpoint, from `INTEGRATION_TYPE_VALUES`:
/// "MOCK: for integrating the route or method request with API Gateway as a 'loopback'
/// endpoint without invoking any backend."
pub const MOCK_INTEGRATION_TYPE: &str = "MOCK";

/// The `ConnectionType` of a private integration, from `CONNECTION_TYPE_VALUES`: the
/// one that connects through a VPC link and so needs the link's id.
pub const VPC_LINK_CONNECTION_TYPE: &str = "VPC_LINK";

/// Every `IntegrationType` an api whose protocol is not WebSocket may be associated
/// with: the values of the modelled enum that [`WEBSOCKET_ONLY_INTEGRATION_TYPES`] does
/// not claim, in the enum's own order.
///
/// Those are exactly the two the member documents as proxy integrations — "also
/// referred to as a Lambda proxy integration" (`AWS_PROXY`) and "also referred to as the
/// HTTP proxy integration" (`HTTP_PROXY`) — which is what the refusal below calls them.
pub fn non_websocket_integration_types() -> Vec<&'static str> {
    wire::INTEGRATION_TYPE_VALUES
        .iter()
        .copied()
        .filter(|value| !WEBSOCKET_ONLY_INTEGRATION_TYPES.contains(value))
        .collect()
}

/// `Ok(())`, or why an api of this `ProtocolType` cannot be associated with an
/// integration of this `IntegrationType` — the message for the operation's
/// `BadRequestException`.
///
/// One rule: an api supports only the integration types its own protocol has behaviour
/// for, and the three the member's documentation marks "Supported only for WebSocket
/// APIs" ([`WEBSOCKET_ONLY_INTEGRATION_TYPES`]) are therefore not ones an api of any
/// other protocol may carry. The refusal names the types that api MAY be associated
/// with (see [`non_websocket_integration_types`]) rather than the value it refused, so
/// what the caller has to send instead is in the message.
///
/// Every writer of an integration routes through here — the create, the update and an
/// import — so the gate and its wording cannot differ between them.
pub fn validate_integration_type_for_protocol(
    protocol_type: &str,
    integration_type: &str,
) -> Result<(), String> {
    if protocol_type == WEBSOCKET_PROTOCOL_TYPE
        || !WEBSOCKET_ONLY_INTEGRATION_TYPES.contains(&integration_type)
    {
        return Ok(());
    }
    Err(format!(
        "Currently, an API with a protocol type of {protocol_type} may only be \
         associated with proxy integrations ({})",
        non_websocket_integration_types().join(", ")
    ))
}

/// `Ok(())`, or why this integration's configuration cannot hold together — the message
/// for the operation's `BadRequestException`.
///
/// The rules an integration RECORD has to satisfy, whatever wrote it, so a create and a
/// later update cannot disagree about them; an update checks the MERGED record, which is
/// the one the api would carry:
///
/// * an `IntegrationSubtype` names the AWS service action an AWS_PROXY integration
///   invokes ("Supported only for HTTP API AWS_PROXY integrations. Specifies the AWS
///   service action to invoke", `CreateIntegration$IntegrationSubtype` — the protocol
///   half of that clause is the protocol gate's, this is the type half);
/// * an `IntegrationUri` is needed only when the URI is what names what the integration
///   invokes: `MOCK` invokes nothing and a subtype names the action itself;
/// * the payload format version is the one the type supports (see
///   [`validate_payload_format_version`]);
/// * "The ID of the VPC link for a private integration" is what a `VPC_LINK` connection
///   connects through, and it means nothing without one;
/// * a template selection expression names only variables the service evaluates (see
///   [`validate_selection_expression`]).
pub fn validate_integration(
    integration_type: &str,
    integration_subtype: Option<&str>,
    integration_uri: Option<&str>,
    payload_format_version: Option<&str>,
    connection_type: &str,
    connection_id: Option<&str>,
    template_selection_expression: Option<&str>,
) -> Result<(), String> {
    if integration_subtype.is_some() && integration_type != AWS_PROXY_INTEGRATION_TYPE {
        return Err(format!(
            "IntegrationSubtype is supported only for integrations of type \
             {AWS_PROXY_INTEGRATION_TYPE}, and this integration's IntegrationType is \
             {integration_type}"
        ));
    }
    if integration_uri.is_none_or(str::is_empty)
        && integration_type != MOCK_INTEGRATION_TYPE
        && integration_subtype.is_none()
    {
        return Err(format!(
            "IntegrationUri is required for an integration of type {integration_type}."
        ));
    }
    validate_payload_format_version(
        integration_type,
        integration_subtype,
        payload_format_version,
    )?;
    if connection_type == VPC_LINK_CONNECTION_TYPE && connection_id.is_none_or(str::is_empty) {
        return Err(format!(
            "ConnectionId is required for a connection of type {VPC_LINK_CONNECTION_TYPE}."
        ));
    }
    if let Some(expression) = template_selection_expression {
        validate_selection_expression(expression)?;
    }
    Ok(())
}

/// The name of the path variable a route-key path segment declares, or `None` for a
/// literal segment. `{id}` names `id` and the greedy `{proxy+}` names `proxy+`.
fn route_key_path_variable(segment: &str) -> Option<&str> {
    segment.strip_prefix('{')?.strip_suffix('}')
}

/// The method and the path segments of a route key of the `METHOD /path` form, or `None`
/// for a key of any other form — a WebSocket key (`$connect`, `$default`,
/// `sendmessage`) names no path, so it declares no path variables at all.
fn route_key_parts(route_key: &str) -> Option<(&str, Vec<&str>)> {
    let (method, path) = route_key.split_once(' ')?;
    let path = path.strip_prefix('/')?;
    Some((method, path.split('/').collect()))
}

/// `Ok(())`, or why this route key cannot live beside the api's existing ones — the
/// message for the operation's `BadRequestException`.
///
/// One rule: within a method's path space a position that is a path VARIABLE has ONE
/// name. Two route keys of the same method that agree on every earlier segment therefore
/// cannot declare differently named variables at the same position, because a request
/// matching both would have to bind two different path parameters at once. A literal
/// beside a variable is not a conflict (the more specific key simply wins), and the two
/// keys part company there, so nothing deeper can conflict either.
///
/// Shared by every writer of a route key, so the requirement cannot differ between the
/// create and a later update. A duplicate key is a different refusal, `ConflictException`.
pub fn validate_route_key_variables<'a>(
    route_key: &str,
    existing_keys: impl Iterator<Item = &'a str>,
) -> Result<(), String> {
    let Some((method, segments)) = route_key_parts(route_key) else {
        return Ok(());
    };
    for existing in existing_keys {
        let Some((other_method, other_segments)) = route_key_parts(existing) else {
            continue;
        };
        // One method's path space at a time: two methods are routed separately, so each
        // binds its own path parameters.
        if method != other_method {
            continue;
        }
        for (segment, other) in segments.iter().zip(other_segments.iter()) {
            match (
                route_key_path_variable(segment),
                route_key_path_variable(other),
            ) {
                (Some(name), Some(other_name)) if name != other_name => {
                    // The message is built from the request: the key it asked for and
                    // the key already on the api whose variable sits at that level.
                    return Err(format!(
                        "The provided route key \"{route_key}\" has a conflicting variable \
                         on the same hierarchical level as \"{existing}\""
                    ));
                }
                // The same variable at the same position, or the same literal: the keys
                // still share a path space, so keep comparing.
                (Some(_), Some(_)) => continue,
                _ if segment == other => continue,
                // A literal beside a variable, or two different literals: the keys part
                // company here.
                _ => break,
            }
        }
    }
    Ok(())
}

/// The sentence this service refuses a route whose `AuthorizerId` names no authorizer of
/// its api with.
///
/// The refusal is about the ASSOCIATION, not about the id — an id a route may not name is
/// one no authorizer of that api carries — so the sentence states the requirement rather
/// than echoing the id back.
pub const UNRESOLVED_AUTHORIZER_MESSAGE: &str = "Invalid authorizer ID specified. Setting \
    the authorization type to CUSTOM or COGNITO_USER_POOLS requires a valid authorizer.";

/// The sentence this service answers `NotFoundException` with for an `AuthorizerId` no
/// authorizer of the addressed api carries.
///
/// API Gateway answers an unknown authorizer identifier with this sentence alone: unlike
/// the api-level [`invalid_identifier_message`], it names neither the account nor the
/// identifier the request carried.
pub const INVALID_AUTHORIZER_IDENTIFIER_MESSAGE: &str = "Invalid Authorizer identifier specified";

/// The message this service answers `NotFoundException` with for a request naming a
/// resource no record holds: the sentence API Gateway answers an unknown identifier
/// with, for the resource named ("Invalid API identifier specified
/// {account}:{identifier}").
pub fn invalid_identifier_message(resource: &str, id: &str) -> String {
    format!(
        "Invalid {resource} identifier specified {}:{id}",
        ::engine::emulator_account_id()
    )
}

/// The `IntegrationId` a route target names, when it addresses an integration of the
/// same api (`integrations/{IntegrationId}`). `None` for a target of any other form — a
/// target this service does not resolve to an integration references none of them.
pub fn target_integration_id(target: &str) -> Option<&str> {
    target.strip_prefix(INTEGRATION_TARGET_PREFIX)
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
/// `Ok(())` or the message for the operation's `BadRequestException`, which is the one
/// sentence the whole class is refused with (see [`INVALID_MODEL_SCHEMA_MESSAGE`]).
pub fn validate_model_schema(content_type: Option<&str>, schema: &str) -> Result<(), String> {
    if !is_json_media_type(content_type) {
        return Ok(());
    }
    let document = ::serde_json::from_str::<::serde_json::Value>(schema).ok();
    if document.is_some_and(|document| document.is_object()) {
        return Ok(());
    }
    Err(INVALID_MODEL_SCHEMA_MESSAGE.to_string())
}

/// The sentence API Gateway refuses a `Schema` that is no schema document with.
///
/// The service answers the RESULT of validating the model, and a document it could not
/// read as a schema yields that result's one error and no warnings. It is one sentence
/// for the whole class: it names nothing from the request, so neither the schema nor any
/// reader's diagnostic (a parser's position, the JSON type the document turned out to
/// be) is echoed back, and the refusal a caller sees does not vary with either.
pub const INVALID_MODEL_SCHEMA_MESSAGE: &str = "Invalid model specified: Validation \
    Result: warnings : [], errors : [Invalid model schema specified]";

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

/// The sentence API Gateway refuses a `CreateStage` naming a stage its api already
/// holds with.
///
/// A stage is identified by its name within its api, so the conflict is that the name
/// is taken; the sentence states that and names nothing from the request, so it is the
/// same for every name.
pub const STAGE_ALREADY_EXISTS_MESSAGE: &str = "Stage already exists";

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

/// The most portals one account may hold, after which CreatePortal is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: API Gateway publishes
/// per-account quotas for its other top-level resources (600 HTTP/WebSocket APIs, see
/// [`MAX_APIS`]) but none for portals, so this takes a smaller bound of its own. What
/// it buys is that no loop of CreatePortal calls can grow this store without end.
/// `TooManyRequestsException` ("A limit has been exceeded") is the modelled error the
/// operation refuses with. Do not cite this as a published portals quota.
pub const MAX_PORTALS: usize = 100;

/// The `PublishStatus` a portal carries while it serves nothing: the value
/// [`Portal`] is created with, since a portal that has never been published is not
/// serving its content to anyone.
///
/// `PublishStatus` is exactly `PUBLISHED | PUBLISH_IN_PROGRESS | PUBLISH_FAILED |
/// DISABLE_IN_PROGRESS | DISABLE_FAILED | DISABLED`
/// (`smithy_query(query="shape", shape="PublishStatus")`) — it has no "never published"
/// value, and `DISABLED` is its one terminal not-serving value, the status a portal also
/// settles in once `DisablePortal` has run.
pub const PUBLISH_STATUS_DISABLED: &str = "DISABLED";

/// The `PublishStatus` of a portal that is serving its content, out of
/// `PUBLISH_STATUS_VALUES`: the value a portal settles in once the publish
/// `PublishPortal` started has come due.
pub const PUBLISH_STATUS_PUBLISHED: &str = "PUBLISHED";

/// The `PublishStatus` a portal reports while the publish `PublishPortal` started is
/// still being built, out of `PUBLISH_STATUS_VALUES`. Not a terminal value: the portal
/// leaves it for [`PUBLISH_STATUS_PUBLISHED`] once [`PORTAL_BUILD`] has passed.
pub const PUBLISH_STATUS_PUBLISH_IN_PROGRESS: &str = "PUBLISH_IN_PROGRESS";

/// The `PublishStatus` a portal reports while the publication `DisablePortal` is taking
/// down is still being removed, out of `PUBLISH_STATUS_VALUES`. Not a terminal value:
/// the portal leaves it for [`PUBLISH_STATUS_DISABLED`] once [`PORTAL_BUILD`] has
/// passed.
pub const PUBLISH_STATUS_DISABLE_IN_PROGRESS: &str = "DISABLE_IN_PROGRESS";

/// The `PreviewStatus` a portal's preview reports while it is still being built, out of
/// `PREVIEW_STATUS_VALUES` (`PREVIEW_FAILED | PREVIEW_IN_PROGRESS | PREVIEW_READY`,
/// `smithy_query(query="shape", shape="PreviewStatus")`).
pub const PREVIEW_STATUS_PREVIEW_IN_PROGRESS: &str = "PREVIEW_IN_PROGRESS";

/// The `PreviewStatus` of a preview that has finished building and can be opened, out
/// of `PREVIEW_STATUS_VALUES`.
pub const PREVIEW_STATUS_PREVIEW_READY: &str = "PREVIEW_READY";

/// How long one server-managed portal build takes: the span a publish, a disable or a
/// preview is still in flight for, after which the clock owes the portal the status
/// that build settles it in.
///
/// A CHOSEN span, not one AWS documents — AWS takes as long as building and
/// distributing a portal's content takes. What matters is that it is never zero, so a
/// caller that publishes a portal and immediately reads it back still sees
/// [`PUBLISH_STATUS_PUBLISH_IN_PROGRESS`] and the refusal that status carries, and that
/// nothing sleeps for it: the due instant is stored and the status derived from the
/// clock on every read.
pub const PORTAL_BUILD: Duration = Duration::from_millis(500);

/// A publish or a disable of a portal that the SERVICE carries out on its own once the
/// call that asked for it has answered.
///
/// Holds the two statuses the build moves between and the instant it is due, so the
/// status a portal reports is derived from the clock rather than written by whoever
/// reads it; see [`Portal::settle`].
#[derive(Debug, Clone)]
pub struct PortalTransition {
    /// The `PublishStatus` the portal reports while this build is still in flight.
    pub in_progress: &'static str,
    /// The `PublishStatus` the portal settles in once this build is due.
    pub settled: &'static str,
    /// When this build is due to finish.
    pub due_at: SystemTime,
    /// The publication this build makes, reported once it has settled, or `None` for a
    /// disable: taking a publication down does not make one, so it leaves the record of
    /// the last one alone.
    pub published: Option<PortalPublication>,
    /// Whether this build is building the portal's content: a publish is, a disable is
    /// not. Decided by [`PortalBuild::builds_content`] and carried here so
    /// [`Portal::content_build_in_flight`] can answer it off the build in flight.
    pub builds_content: bool,
}

/// What a settled publish reports as the portal's last publication.
#[derive(Debug, Clone)]
pub struct PortalPublication {
    /// The instant the portal was published — the instant the publish finished, which
    /// is when the content it built went live.
    pub at: SystemTime,
    /// The description the publish request carried: "When the portal is published, this
    /// description becomes the last published description"
    /// (`PublishPortal$Description`). `None` for a publish that carried none, which
    /// publishes a version with no description.
    pub description: Option<String>,
}

/// What one of the three portal build calls asks the service to do.
///
/// Each names what it NEEDS before it can start — the portal's publication still, its
/// content build free, or nothing at all — which is what decides whether it can start
/// while the service is already busy with this portal (see [`Portal::start_build`]).
#[derive(Debug, Clone)]
pub enum PortalBuild {
    /// `PublishPortal`: build the portal's content and serve it, recording the
    /// description the request carried as the last published one.
    Publish { description: Option<String> },
    /// `DisablePortal`: take the publication down, after which the portal serves
    /// nothing.
    Disable,
    /// `PreviewPortal`: build a preview of the portal's content, which changes nothing
    /// about what the portal publishes.
    Preview,
}

impl PortalBuild {
    /// Whether this build needs the portal's publication to be STILL before it can
    /// start: a publish does, since publishing on top of a publication change still in
    /// flight would leave two answers for what the portal serves.
    ///
    /// A disable does not: taking the publication down is a revocation, which the
    /// service applies at once and which supersedes a publish still in flight rather
    /// than queueing behind it.
    pub fn needs_a_still_publication(&self) -> bool {
        matches!(self, PortalBuild::Publish { .. })
    }

    /// Whether this build builds the portal's CONTENT, so it both occupies the content
    /// build and cannot start while another content build is still in flight: a publish
    /// builds the content it serves and a preview builds the content it shows, while a
    /// disable builds nothing — it only stops serving what is already built.
    pub fn builds_content(&self) -> bool {
        matches!(self, PortalBuild::Publish { .. } | PortalBuild::Preview)
    }
}

/// A persisted `Portal` resource.
///
/// An account-level resource: a portal is not owned by an api, and the resources it
/// references (its portal products) are named by ARN rather than nested. The record
/// keeps what the create RESOLVED — the id and ARN it minted, the endpoint configuration
/// it derived from the request, the instant it was last modified — so every reader of
/// the portal reports one value, and [`Portal::to_wire`] projects it onto the modelled
/// `PortalSummary` shape every portal reader serializes from.
#[derive(Debug, Clone)]
pub struct Portal {
    pub portal_id: String,
    pub portal_arn: String,
    /// The authorization the create accepted: exactly one of its alternatives, see
    /// [`validate_exclusive_choice`].
    pub authorization: wire::Authorization,
    /// The endpoint configuration the create RESOLVED: the request names the domain
    /// (its own, or API Gateway's default) and the response reports the default domain
    /// name and hosted zone id the service generated for it.
    pub endpoint_configuration: wire::EndpointConfigurationResponse,
    pub included_portal_product_arns: Vec<String>,
    /// The logo image URI the caller supplied, if any. Not a member of any modelled
    /// portal RESPONSE shape — the logo is reported through
    /// `PortalContent$Theme$LogoLastUploaded` — so the record keeps it for the readers
    /// that serve the portal's content.
    pub logo_uri: Option<String>,
    pub portal_content: wire::PortalContent,
    pub rum_app_monitor_name: Option<String>,
    /// The TERMINAL `PublishStatus` this portal has settled in, out of
    /// `PUBLISH_STATUS_VALUES`: [`PUBLISH_STATUS_DISABLED`] for a portal that has never
    /// been published. An `..._IN_PROGRESS` status is never stored here — it belongs to
    /// a build still in flight, which [`Portal::transition`] holds and
    /// [`Portal::settle`] retires.
    pub publish_status: String,
    pub last_modified: SystemTime,
    /// Set only once the portal has been published; `None` until then, so nothing is
    /// reported for a portal that never was.
    pub last_published: Option<SystemTime>,
    pub last_published_description: Option<String>,
    /// "Error information for failed portal operations" — `None` while nothing has
    /// failed.
    pub status_exception: Option<wire::StatusException>,
    /// The publish or disable the service is building, or the last one it built; `None`
    /// until one is started. See [`Portal::settle`].
    pub transition: Option<PortalTransition>,
    /// When the preview this portal's last `PreviewPortal` started is due to be
    /// ready; `None` until one runs, so nothing is reported for a portal with no
    /// preview. The `PreviewStatus` is derived from it on every read, so no stored copy
    /// of it can go stale.
    pub preview_ready_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
}

impl Portal {
    /// The change to this portal's PUBLICATION that is still in flight as of `now`,
    /// named by the `PublishStatus` it reports while it runs, or `None` when the
    /// publication is not moving.
    pub fn publication_change_in_flight(&self, now: SystemTime) -> Option<&'static str> {
        self.transition
            .as_ref()
            .filter(|transition| now < transition.due_at)
            .map(|transition| transition.in_progress)
    }

    /// The build of this portal's CONTENT that is still in flight as of `now`, named by
    /// the status it reports while it runs — the `PublishStatus` of a publish, which
    /// builds the content it serves, or the `PreviewStatus` of a preview — or `None` when
    /// nothing is being built.
    pub fn content_build_in_flight(&self, now: SystemTime) -> Option<&'static str> {
        let publishing = self
            .transition
            .as_ref()
            .filter(|transition| now < transition.due_at && transition.builds_content)
            .map(|transition| transition.in_progress);
        let previewing = self
            .preview_ready_at
            .filter(|ready_at| now < *ready_at)
            .map(|_| PREVIEW_STATUS_PREVIEW_IN_PROGRESS);
        publishing.or(previewing)
    }

    /// Materialise a publish or disable the CLOCK has already made due: the status it
    /// settles the portal in, and the publication it made, become the portal's own and
    /// the build is retired.
    ///
    /// Owed at the due instant whether or not anyone called, so the change is the
    /// clock's rather than this caller's. Readers never have to run it against the
    /// stored record — [`Portal::to_wire`] settles a copy — so reading a portal writes
    /// nothing.
    pub fn settle(&mut self, now: SystemTime) {
        if self
            .transition
            .as_ref()
            .is_none_or(|transition| now < transition.due_at)
        {
            return;
        }
        let Some(transition) = self.transition.take() else {
            return;
        };
        self.publish_status = transition.settled.to_string();
        if let Some(published) = transition.published {
            self.last_published = Some(published.at);
            self.last_published_description = published.description;
        }
    }

    /// Start one server-managed build on this portal, due [`PORTAL_BUILD`] from `now`.
    ///
    /// A build starts only when what it needs is free: a publish needs the publication
    /// still AND the content build free, a preview needs the content build free, and a
    /// disable needs nothing — it revokes what the portal serves, so it supersedes a
    /// publish still in flight (which then publishes nothing) instead of queueing behind
    /// it. So a preview and a disable run side by side, while a publish runs alone.
    ///
    /// `Err` carries the status of the build already in flight, for the caller's
    /// `ConflictException` — "the requested operation would cause a conflict with the
    /// current state of a service resource associated with the request" — and leaves the
    /// record exactly as it was. The refusal lifts of its own accord once the build this
    /// service already owes comes due.
    pub fn start_build(&mut self, now: SystemTime, build: PortalBuild) -> Result<(), &'static str> {
        self.settle(now);
        if build.needs_a_still_publication() {
            if let Some(in_flight) = self.publication_change_in_flight(now) {
                return Err(in_flight);
            }
        }
        if build.builds_content() {
            if let Some(in_flight) = self.content_build_in_flight(now) {
                return Err(in_flight);
            }
        }
        let builds_content = build.builds_content();
        let due_at = now + PORTAL_BUILD;
        match build {
            PortalBuild::Publish { description } => {
                self.transition = Some(PortalTransition {
                    in_progress: PUBLISH_STATUS_PUBLISH_IN_PROGRESS,
                    settled: PUBLISH_STATUS_PUBLISHED,
                    due_at,
                    published: Some(PortalPublication {
                        at: due_at,
                        description,
                    }),
                    builds_content,
                });
            }
            PortalBuild::Disable => {
                self.transition = Some(PortalTransition {
                    in_progress: PUBLISH_STATUS_DISABLE_IN_PROGRESS,
                    settled: PUBLISH_STATUS_DISABLED,
                    due_at,
                    published: ::std::option::Option::None,
                    builds_content,
                });
            }
            PortalBuild::Preview => self.preview_ready_at = Some(due_at),
        }
        Ok(())
    }

    /// Project the record onto the modelled `PortalSummary` shape as of `now` — the
    /// shape that carries every member the portal readers report, so each of them
    /// projects from this one value.
    pub fn to_wire(&self, now: SystemTime) -> wire::PortalSummary {
        // A read must not change what a later caller sees, so the projection settles a
        // COPY: a build the clock has already made due is reported as settled while the
        // stored record is left for the next writer to settle.
        let mut settled = self.clone();
        settled.settle(now);
        // Every build the clock owed is retired above, so a transition still held is one
        // still in flight and the portal reports the status it carries while it runs.
        let publish_status = match &settled.transition {
            Some(transition) => transition.in_progress.to_string(),
            ::std::option::Option::None => settled.publish_status.clone(),
        };
        let preview = settled.preview_ready_at.map(|ready_at| wire::Preview {
            preview_status: if now < ready_at {
                PREVIEW_STATUS_PREVIEW_IN_PROGRESS
            } else {
                PREVIEW_STATUS_PREVIEW_READY
            }
            .to_string(),
            preview_url: Some(portal_preview_url(&settled.endpoint_configuration)),
            // Nothing failed, so there is no exception to explain.
            status_exception: ::std::option::Option::None,
        });
        wire::PortalSummary {
            authorization: settled.authorization.clone(),
            endpoint_configuration: settled.endpoint_configuration.clone(),
            included_portal_product_arns: settled.included_portal_product_arns.clone(),
            last_modified: to_timestamp(settled.last_modified),
            last_published: settled.last_published.map(to_timestamp),
            last_published_description: settled.last_published_description.clone(),
            portal_arn: settled.portal_arn.clone(),
            portal_content: settled.portal_content.clone(),
            portal_id: settled.portal_id.clone(),
            preview,
            publish_status: Some(publish_status),
            rum_app_monitor_name: settled.rum_app_monitor_name.clone(),
            status_exception: settled.status_exception.clone(),
            tags: Some(
                settled
                    .tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }
}

/// The most portal products one account may hold, after which CreatePortalProduct is
/// refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: it keeps a loop of
/// CreatePortalProduct calls from growing this store without end, and is set to the
/// per-parent bound this service already uses for the other collections it mints ids
/// into ([`MAX_MODELS_PER_API`]). `TooManyRequestsException` ("A limit has been
/// exceeded") is the modelled error the operation refuses with. Do not cite this as a
/// published portal-products quota.
pub const MAX_PORTAL_PRODUCTS: usize = 300;

/// The `PolicyDocument` a portal product's sharing policy reads as while the product
/// holds no policy of its own: the empty JSON policy document.
///
/// Every portal product HAS a sharing policy to report — a product nothing has shared is
/// one whose policy grants nothing, not one with no policy sub-resource — so a product
/// created without one, and one whose policy `DeletePortalProductSharingPolicy` reset,
/// reports a policy document carrying no statements rather than no document at all.
pub const EMPTY_SHARING_POLICY: &str = "{}";

/// A persisted `PortalProduct` resource.
///
/// "Creates a new portal product" (`CreatePortalProduct`): an account-level resource,
/// NOT a child of an api, so it lives beside
/// [`ComAmazonawsApigatewayv2Apigatewayv2State::apis`] rather than inside one.
#[derive(Debug, Clone)]
pub struct PortalProduct {
    pub portal_product_id: String,
    /// The ARN every reader reports, built once at create time by
    /// [`portal_product_arn`] from the Region the creating request addressed: an ARN
    /// names the Region its resource was created in, so it cannot be rebuilt per read
    /// from whatever Region a later caller signs for.
    pub portal_product_arn: String,
    pub display_name: String,
    pub description: Option<String>,
    /// "The visual ordering of the product pages and product REST endpoint pages in a
    /// published portal" (`PortalProduct$DisplayOrder`). A freshly created product owns
    /// no pages, so it has no ordering until one is configured.
    pub display_order: Option<wire::DisplayOrder>,
    /// "The timestamp when the portal product was last modified"
    /// (`PortalProduct$LastModified`) — the creation instant until something rewrites
    /// the record.
    pub last_modified: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// "The product sharing policy" (`PutPortalProductSharingPolicy$PolicyDocument`),
    /// the policy document the product's own sharing-policy sub-resource
    /// (`/v2/portalproducts/{PortalProductId}/sharingpolicy`) holds. `None` while the
    /// product carries no sharing policy — the state a freshly created product is in and
    /// the one `DeletePortalProductSharingPolicy` puts it back into, so the policy's
    /// absence is a value of this member rather than the absence of the product.
    pub sharing_policy: Option<String>,
    /// This product's product REST endpoint pages, keyed by
    /// `ProductRestEndpointPageId`. A page is addressed UNDER its product
    /// (`/v2/portalproducts/{PortalProductId}/productrestendpointpages/{id}`), so the
    /// product OWNS its pages the way an [`Api`] owns its routes: removing the product
    /// takes its pages with it, and no second map can drift out of step with it.
    /// `BTreeMap` for the stable key order a paginated listing resumes through.
    pub product_rest_endpoint_pages: BTreeMap<String, ProductRestEndpointPage>,
    /// Drives [`PortalProduct::mint_product_rest_endpoint_page_id`]. Per-product, for
    /// the same reason [`Api::route_id_seed`] is per-api: the id only has to be unique
    /// within the product that owns the page.
    pub product_rest_endpoint_page_id_seed: u64,
}

/// The members `CreatePortalProduct`, `GetPortalProduct` and `UpdatePortalProduct`
/// each report for one portal product — the three modelled responses carry exactly the
/// same set — so [`PortalProduct::to_wire`] projects the record ONCE and every reader
/// serializes that projection rather than assembling its own.
#[derive(Debug, Clone)]
pub struct PortalProductView {
    pub description: Option<String>,
    pub display_name: Option<String>,
    pub display_order: Option<wire::DisplayOrder>,
    pub last_modified: Option<::aws_smithy_types::DateTime>,
    pub portal_product_arn: Option<String>,
    pub portal_product_id: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

impl PortalProduct {
    /// The `PolicyDocument` this product's sharing policy reports: the document stored
    /// by its last `PutPortalProductSharingPolicy`, or [`EMPTY_SHARING_POLICY`] while the
    /// product holds none.
    pub fn sharing_policy_document(&self) -> String {
        self.sharing_policy
            .clone()
            .unwrap_or_else(|| EMPTY_SHARING_POLICY.to_string())
    }
    /// Mint a fresh `ProductRestEndpointPageId` for this product: ten lowercase
    /// alphanumerics, never one this product already minted. The member targets
    /// `__stringMin10Max30PatternAZ09` (len 10..30, pattern `^[a-z0-9]+$`), which
    /// [`mint_id`]'s ten-character lowercase alphanumeric id satisfies by construction.
    pub fn mint_product_rest_endpoint_page_id(&mut self) -> String {
        self.product_rest_endpoint_page_id_seed =
            self.product_rest_endpoint_page_id_seed.wrapping_add(1);
        mint_id(self.product_rest_endpoint_page_id_seed)
    }

    /// Project the record onto the members every portal-product reader reports.
    pub fn to_wire(&self) -> PortalProductView {
        PortalProductView {
            description: self.description.clone(),
            display_name: Some(self.display_name.clone()),
            display_order: self.display_order.clone(),
            last_modified: Some(to_timestamp(self.last_modified)),
            portal_product_arn: Some(self.portal_product_arn.clone()),
            portal_product_id: Some(self.portal_product_id.clone()),
            tags: Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            ),
        }
    }
}

/// The most product REST endpoint pages one portal product may hold, after which
/// CreateProductRestEndpointPage is refused.
///
/// An EMULATOR growth bound, not a figure AWS publishes: it keeps a loop of
/// CreateProductRestEndpointPage calls from growing one product without end, and is set
/// to the per-parent bound this service already uses for the other child collections it
/// mints ids into ([`MAX_ROUTES_PER_API`]). `TooManyRequestsException` ("A limit has
/// been exceeded") is the modelled error the operation refuses with. Do not cite this as
/// a published quota.
pub const MAX_PRODUCT_REST_ENDPOINT_PAGES_PER_PRODUCT: usize = 300;

/// The `Status` a resource carries while the service is still assembling it, from
/// `STATUS_VALUES` (`AVAILABLE | FAILED | IN_PROGRESS`).
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `Status` a resource carries once the service finished assembling it, from
/// `STATUS_VALUES`.
pub const STATUS_AVAILABLE: &str = "AVAILABLE";

/// How long a created product REST endpoint page reports [`STATUS_IN_PROGRESS`] before
/// it reports [`STATUS_AVAILABLE`]: the page's content is generated from the REST
/// endpoint it documents, which is not done when the create call answers — the reason
/// the modelled response carries a `Status` and a `StatusException` at all.
///
/// AWS documents no duration for it, so this span is the emulator's own, chosen short
/// enough not to slow a caller down and long enough that a caller reading straight after
/// the create still observes the transient status. It is the span
/// [`VPC_LINK_CREATE_SPAN`] uses for the same kind of transition.
pub const PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN: Duration = Duration::from_millis(500);

/// The `TryItState` a product REST endpoint page gets when the request names none, from
/// `TRY_IT_STATE_VALUES` (`DISABLED | ENABLED`).
///
/// The member is REQUIRED on the modelled page summary, so a page always reports one,
/// and the request may leave it out; `DISABLED` is the CHOSEN default, since a caller
/// that did not ask for the portal's try-it console to invoke this endpoint has not asked
/// for it to be invocable from the portal.
pub const DEFAULT_TRY_IT_STATE: &str = "DISABLED";

/// The resource portion a product REST endpoint page's ARN carries after its product's:
/// API Gateway names a resource by its own REST path, and a page's path is
/// `/portalproducts/{portalProductId}/productrestendpointpages/{pageId}`.
pub const PRODUCT_REST_ENDPOINT_PAGE_RESOURCE_SEGMENT: &str = "/productrestendpointpages/";

/// The ARN of a stored product REST endpoint page.
///
/// Built like every other ARN this store names — through [`::engine::emulator_arn`] under
/// [`ARN_SERVICE`], with the resource portion the page's own REST path (see
/// [`PRODUCT_REST_ENDPOINT_PAGE_RESOURCE_SEGMENT`]) — so it names the page's product as
/// well as the page, and satisfies the modelled `len 20..2048` of
/// `ProductRestEndpointPageArn`.
pub fn product_rest_endpoint_page_arn(
    region: &str,
    portal_product_id: &str,
    product_rest_endpoint_page_id: &str,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{PORTAL_PRODUCT_RESOURCE_PREFIX}{portal_product_id}\
             {PRODUCT_REST_ENDPOINT_PAGE_RESOURCE_SEGMENT}{product_rest_endpoint_page_id}"
        ),
    )
}

/// The longest `Endpoint` a page's display content may report: the modelled
/// `EndpointDisplayContentResponse$Endpoint` is `len 1..1024`.
pub const DISPLAY_CONTENT_ENDPOINT_MAX_LENGTH: usize = 1024;

/// The identifier parts of the REST endpoint a `RestEndpointIdentifier` names, or the
/// refusal of one that names no endpoint.
///
/// `RestEndpointIdentifier` is a structure with exactly one alternative,
/// `IdentifierParts` ("The identifier parts of the REST endpoint identifier"), so an
/// identifier carrying none identifies nothing to document — incomplete input, which is
/// what `BadRequestException` reports. The wording is
/// [`exclusive_choice_required`]'s, so this refusal reads like the service's other
/// one-of refusals.
pub fn rest_endpoint_identifier_parts(
    identifier: &wire::RestEndpointIdentifier,
) -> Result<&wire::IdentifierParts, String> {
    match identifier.identifier_parts.as_ref() {
        Some(parts) => Ok(parts),
        None => Err(exclusive_choice_required(
            "RestEndpointIdentifier",
            &["IdentifierParts"],
        )),
    }
}

/// The URL a consumer invokes the REST endpoint a page documents on.
///
/// A REST endpoint is identified by its REST API, stage, method and path
/// (`IdentifierParts`), and API Gateway serves it on that API's default execute-api
/// endpoint under the stage and the path — the host
/// `{api-id}.execute-api.{region}.amazonaws.com` [`execute_api_endpoint`] builds for
/// every api of this service. The method is not part of a URL, so it does not appear
/// here.
pub fn rest_endpoint_invoke_url(parts: &wire::IdentifierParts, region: &str) -> String {
    // The path is written with a leading separator (`/orders`), and the stage already
    // contributes one, so a leading separator is dropped rather than doubled.
    let path = parts.path.strip_prefix('/').unwrap_or(&parts.path);
    format!(
        "{}/{}/{path}",
        execute_api_endpoint(&parts.rest_api_id, region, HTTP_PROTOCOL_TYPE),
        parts.stage
    )
}

/// Resolve the display content a product REST endpoint page reports from the content the
/// request asked for.
///
/// "Contains any values that override the default configuration generated from API
/// Gateway" (`DisplayContentOverrides`): each member of the page's display content is
/// the value the request overrode it with, or the default API Gateway derives from the
/// REST endpoint the page documents. `Endpoint` is required on the response, and its
/// default is that endpoint's own invoke URL ("By default, API Gateway uses the default
/// execute API endpoint. You can provide a custom domain to override this value",
/// `DisplayContentOverrides$Endpoint`). `Body` ("By default, this is the documentation of
/// your REST API from API Gateway") and `OperationName` default to documentation that
/// lives in the REST API itself — a resource of the REST (API Gateway v1) API this
/// emulator does not hold — so nothing is invented for them and they are reported only
/// where the request supplied them.
///
/// `requested` is the request's `DisplayContent`, absent for a request that named none —
/// which says the same as its `None` alternative: this endpoint carries no overrides.
/// `Err` carries the message for the operation's `BadRequestException`.
pub fn resolve_endpoint_display_content(
    region: &str,
    parts: &wire::IdentifierParts,
    requested: Option<&wire::EndpointDisplayContent>,
) -> Result<wire::EndpointDisplayContentResponse, String> {
    let overrides = match requested {
        None => None,
        Some(content) => {
            validate_exclusive_choice(
                "DisplayContent",
                &[
                    ("None", content.none.is_some()),
                    ("Overrides", content.overrides.is_some()),
                ],
            )?;
            content.overrides.as_ref()
        }
    };

    let endpoint = match overrides.and_then(|overrides| overrides.endpoint.clone()) {
        Some(endpoint) => endpoint,
        None => {
            let derived = rest_endpoint_invoke_url(parts, region);
            // An overriding `Endpoint` is bounded by the model (len 1..1024) and refused
            // at the boundary, but a DERIVED one is this service's own value: the parts
            // it is built from are bounded far wider (a `Path` alone is len 1..4096), so
            // a request naming an endpoint whose invoke URL cannot be reported is refused
            // rather than answered with a response the model does not allow.
            if derived.len() > DISPLAY_CONTENT_ENDPOINT_MAX_LENGTH {
                return Err(format!(
                    "Invalid value for RestEndpointIdentifier: the invoke URL of the \
                     identified REST endpoint is {} characters, past the \
                     {DISPLAY_CONTENT_ENDPOINT_MAX_LENGTH} an endpoint may carry.",
                    derived.len()
                ));
            }
            derived
        }
    };

    Ok(wire::EndpointDisplayContentResponse {
        body: overrides.and_then(|overrides| overrides.body.clone()),
        endpoint,
        operation_name: overrides.and_then(|overrides| overrides.operation_name.clone()),
    })
}

/// A persisted `ProductRestEndpointPage` resource: one REST endpoint of a portal
/// product, as a published portal documents it.
///
/// Owned by its [`PortalProduct`] (see
/// [`PortalProduct::product_rest_endpoint_pages`]). The record keeps what the write
/// RESOLVED — the id and ARN it minted, the display content it resolved from the
/// request's overrides and the identified endpoint, the instant the page was last
/// modified — so every reader of the page reports one value.
#[derive(Debug, Clone)]
pub struct ProductRestEndpointPage {
    pub product_rest_endpoint_page_id: String,
    /// Built once at write time by [`product_rest_endpoint_page_arn`] from the Region the
    /// creating request addressed: an ARN names the Region its resource was created in,
    /// so it cannot be rebuilt per read from whatever Region a later caller signs for.
    pub product_rest_endpoint_page_arn: String,
    /// The Region the creating request addressed, kept beside the ARN that names it: a
    /// later WRITE that re-resolves this page's display content derives the identified
    /// endpoint's invoke URL in the page's OWN Region, rather than in whatever Region the
    /// writing caller happened to sign for.
    pub region: String,
    /// The REST endpoint this page documents, as the request identified it.
    pub rest_endpoint_identifier: wire::RestEndpointIdentifier,
    /// The display content this page reports, resolved by
    /// [`resolve_endpoint_display_content`] at every WRITE rather than per read, so the
    /// endpoint it names does not follow the Region a later reader signs for.
    pub display_content: wire::EndpointDisplayContentResponse,
    /// "The timestamp when the product REST endpoint page was last modified" — the
    /// creation instant until something rewrites the record.
    pub last_modified: SystemTime,
    /// One of `TRY_IT_STATE_VALUES`; [`DEFAULT_TRY_IT_STATE`] for a page whose create
    /// named none.
    pub try_it_state: String,
    /// When this page's content finishes being generated, after which it reports
    /// [`STATUS_AVAILABLE`]; before it, [`STATUS_IN_PROGRESS`]. See
    /// [`ProductRestEndpointPage::status`] and
    /// [`PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN`].
    pub available_at: Instant,
}

/// The members `CreateProductRestEndpointPage`, `GetProductRestEndpointPage` and
/// `UpdateProductRestEndpointPage` each
/// report for one page — the Get response carries the create's members plus
/// `RawDisplayContent` — so [`ProductRestEndpointPage::to_wire`] projects the record
/// ONCE and every reader serializes that projection rather than assembling its own.
#[derive(Debug, Clone)]
pub struct ProductRestEndpointPageView {
    pub display_content: Option<wire::EndpointDisplayContentResponse>,
    pub last_modified: Option<::aws_smithy_types::DateTime>,
    pub product_rest_endpoint_page_arn: Option<String>,
    pub product_rest_endpoint_page_id: Option<String>,
    pub rest_endpoint_identifier: Option<wire::RestEndpointIdentifier>,
    pub status: Option<String>,
    pub status_exception: Option<wire::StatusException>,
    pub try_it_state: Option<String>,
}

impl ProductRestEndpointPage {
    /// The `Status` this page reports now.
    ///
    /// A created page's content is still being generated when the create call answers, so
    /// it is reported [`STATUS_IN_PROGRESS`] until
    /// [`PRODUCT_REST_ENDPOINT_PAGE_CREATE_SPAN`] has passed and [`STATUS_AVAILABLE`]
    /// after it. The transition is one the CLOCK owes, materialised from the stored due
    /// instant rather than by sleeping or by being read, so every reader agrees on it and
    /// a read changes nothing.
    pub fn status(&self) -> &'static str {
        if Instant::now() < self.available_at {
            STATUS_IN_PROGRESS
        } else {
            STATUS_AVAILABLE
        }
    }

    /// The page's content as it was supplied, for the reader that asks for it
    /// (`GetProductRestEndpointPage$IncludeRawDisplayContent`): the body of the page,
    /// before a portal renders it. A page whose body is API Gateway's own documentation
    /// of the REST API carries none here, for the reason
    /// [`resolve_endpoint_display_content`] gives.
    pub fn raw_display_content(&self) -> Option<String> {
        self.display_content.body.clone()
    }

    /// Project the record onto the members every page reader reports.
    pub fn to_wire(&self) -> ProductRestEndpointPageView {
        ProductRestEndpointPageView {
            display_content: Some(self.display_content.clone()),
            last_modified: Some(to_timestamp(self.last_modified)),
            product_rest_endpoint_page_arn: Some(self.product_rest_endpoint_page_arn.clone()),
            product_rest_endpoint_page_id: Some(self.product_rest_endpoint_page_id.clone()),
            rest_endpoint_identifier: Some(self.rest_endpoint_identifier.clone()),
            status: Some(self.status().to_string()),
            // "The status exception information" reports what went WRONG with a page the
            // service could not assemble (`Status` `FAILED`). Nothing failed here, so
            // there is no exception to report rather than an empty one.
            status_exception: None,
            try_it_state: Some(self.try_it_state.clone()),
        }
    }
}

/// A persisted `VpcLink` resource, owned by the account and Region rather than by an
/// api.
#[derive(Debug, Clone)]
pub struct VpcLink {
    pub vpc_link_id: String,
    pub name: String,
    /// The security groups the request named, in the order it named them; empty for a
    /// request that named none. Required on the modelled `VpcLink` shape, so the record
    /// always carries the list even when it is empty.
    pub security_group_ids: Vec<String>,
    /// The subnets the request named, in the order it named them. Required on the
    /// modelled `VpcLink` shape and never empty: a link with no subnet has nowhere to
    /// attach an interface (see [`validate_subnet_ids`]).
    pub subnet_ids: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub created_date: SystemTime,
    /// When this link finishes provisioning, after which it reports
    /// [`VPC_LINK_STATUS_AVAILABLE`]; before it, [`VPC_LINK_STATUS_PENDING`]. See
    /// [`VpcLink::status`] and [`VPC_LINK_CREATE_SPAN`].
    pub available_at: Instant,
}

impl VpcLink {
    /// The `VpcLinkStatus` this link reports now.
    ///
    /// A created link is still being provisioned when the create call answers, so it is
    /// reported [`VPC_LINK_STATUS_PENDING`] until [`VPC_LINK_CREATE_SPAN`] has passed
    /// and [`VPC_LINK_STATUS_AVAILABLE`] after it. The transition is one the CLOCK owes,
    /// materialised from the stored due instant rather than by sleeping or by being
    /// read, so every reader agrees on it and a read changes nothing.
    pub fn status(&self) -> &'static str {
        if Instant::now() < self.available_at {
            VPC_LINK_STATUS_PENDING
        } else {
            VPC_LINK_STATUS_AVAILABLE
        }
    }

    /// The `VpcLinkStatusMessage` this link reports now: "a message summarizing the
    /// cause of the status of the VPC link" (`VpcLink$VpcLinkStatusMessage`).
    ///
    /// AWS documents no wording for it, so the sentence itself is the emulator's own and
    /// is NOT a contract; what IS one is that it summarises the status
    /// [`VpcLink::status`] reports at the same instant.
    pub fn status_message(&self) -> String {
        match self.status() {
            VPC_LINK_STATUS_PENDING => {
                "The VPC link is being provisioned and is not ready to be used by an \
                 integration yet."
                    .to_string()
            }
            // VPC_LINK_STATUS_AVAILABLE: the only other status a stored record reaches.
            _ => "The VPC link is available for use by an integration.".to_string(),
        }
    }

    /// Project the record onto the modelled `VpcLink` shape.
    pub fn to_wire(&self) -> wire::VpcLink {
        wire::VpcLink {
            created_date: Some(to_timestamp(self.created_date)),
            name: self.name.clone(),
            security_group_ids: self.security_group_ids.clone(),
            subnet_ids: self.subnet_ids.clone(),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            vpc_link_id: self.vpc_link_id.clone(),
            vpc_link_status: Some(self.status().to_string()),
            vpc_link_status_message: Some(self.status_message()),
            vpc_link_version: Some(VPC_LINK_VERSION_V2.to_string()),
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

/// Refuse a structure that does not name EXACTLY one of its alternatives.
///
/// Several of this service's input structures are tagged unions modelled as structures
/// whose members are each one alternative — `Authorization` ("Supports Cognito-based
/// user authentication or no authentication": `CognitoConfig | None`) and
/// `EndpointConfigurationRequest` ("Use a default domain provided by API Gateway or
/// provide a fully-qualified domain name that you own": `AcmManaged | None`). The
/// structure's meaning is the ONE alternative it names, so a request naming none has
/// said nothing and one naming several has said two contradictory things; both are
/// incomplete-or-incorrect input, which is what `BadRequestException` reports.
///
/// `alternatives` pairs each alternative's wire name with whether the request carried
/// it, in the order the message should list them. `Err` carries the message for the
/// operation's `BadRequestException`.
pub fn validate_exclusive_choice(
    member: &str,
    alternatives: &[(&str, bool)],
) -> Result<(), String> {
    let names: Vec<&str> = alternatives.iter().map(|(name, _)| *name).collect();
    let named: Vec<&str> = alternatives
        .iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| *name)
        .collect();
    match named.len() {
        1 => Ok(()),
        0 => Err(exclusive_choice_required(member, &names)),
        _ => Err(format!(
            "{member} must specify exactly one of: {}, but specifies {}.",
            names.join(", "),
            named.join(", ")
        )),
    }
}

/// The message a structure naming NO alternative is refused with: the member, and the
/// alternatives it has to choose between. Declared once, so
/// [`validate_exclusive_choice`] and the readers of a one-alternative structure (see
/// [`rest_endpoint_identifier_parts`]) refuse the same shape of input with the same
/// wording.
fn exclusive_choice_required(member: &str, names: &[&str]) -> String {
    format!("{member} must specify exactly one of: {}.", names.join(", "))
}

/// The resource portion every portal-product ARN opens with: API Gateway names a
/// resource by its own REST path, so a portal product's is `/portalproducts/{id}` (see
/// [`portal_product_arn`]). Declared once, so the writer that BUILDS such an ARN and the
/// validator that READS a caller-supplied one cannot drift.
pub const PORTAL_PRODUCT_RESOURCE_PREFIX: &str = "/portalproducts/";

/// Refuse a member that must hold ARNs of a given resource type but holds a value that
/// is not one.
///
/// `IncludedPortalProductArns` names portal products — resources of THIS service's ARN
/// namespace, under the `/portalproducts/` resource path — so a value that is not a
/// well-formed resource ARN, that names another vendor, or that names another of this
/// service's resource types (an `/apis/{id}`, say) addresses nothing the member could
/// include. The parse is [`::service_common::arn::parse_of`], the tree's ONE
/// caller-supplied-ARN parse (it is also what rejects an ARN with an empty partition,
/// vendor or resource); `resource_prefix` is what narrows it to one resource type.
/// `label` names the member in the message; `Err` carries the message for the
/// operation's `BadRequestException`.
pub fn validate_arns_of<'a, I>(
    label: &str,
    vendor: &str,
    resource_prefix: &str,
    arns: I,
) -> Result<(), String>
where
    I: IntoIterator<Item = &'a str>,
{
    for arn in arns {
        // A prefix match alone would accept `…:/portalproducts/` — the resource type with
        // no id, which names no resource — so the remainder after the prefix must be
        // non-empty.
        let names_the_resource = ::service_common::arn::parse_of(arn, vendor).is_some_and(|parsed| {
            parsed
                .resource
                .strip_prefix(resource_prefix)
                .is_some_and(|id| !id.is_empty())
        });
        if !names_the_resource {
            return Err(format!(
                "Invalid ARN specified for {label}: '{arn}'. Expected an ARN of the form \
                 arn:<partition>:{vendor}:<region>:<account>:{resource_prefix}<id>."
            ));
        }
    }
    Ok(())
}

/// The ARN of a stored portal product.
///
/// API Gateway's own ARNs carry the resource portion as the URI path of the resource
/// (`/portalproducts/{portalProductId}`, the `GetPortalProduct` URI without its
/// version prefix), under this service's `arnNamespace`
/// (`smithy_query(query="service_info")` reports `apigateway`). Built through
/// [`::engine::emulator_arn`], so the account segment is the emulator's canonical
/// account rather than a literal: a portal product is an account-owned, shareable
/// resource (`GetPortalProduct$ResourceOwnerAccountId`), so the owning account is part
/// of its name. The namespace is [`ARN_SERVICE`], declared once for every resource this
/// store names.
pub fn portal_product_arn(region: &str, portal_product_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PORTAL_PRODUCT_RESOURCE_PREFIX}{portal_product_id}"),
    )
}

/// The prefix of an Amazon VPC subnet id, the resource type `SubnetIdList`'s members
/// name ("A list of subnet IDs to include in the VPC link", `VpcLink$SubnetIds`).
pub const SUBNET_ID_PREFIX: &str = "subnet-";

/// The prefix of an Amazon EC2 security group id, the resource type
/// `SecurityGroupIdList`'s members name ("A list of security group IDs for the VPC
/// link", `VpcLink$SecurityGroupIds`).
pub const SECURITY_GROUP_ID_PREFIX: &str = "sg-";

/// How many hexadecimal characters an EC2 resource id carries after its prefix.
///
/// From the Amazon EC2 "Resource IDs" documentation: an id is the resource type's prefix
/// followed by 8 characters in the original form, or 17 in the longer form AWS moved to;
/// both forms remain valid, so both lengths are accepted.
pub const EC2_ID_HEX_LENGTHS: &[usize] = &[8, 17];

/// Whether `value` is an EC2 resource id of the type `prefix` names: the prefix followed
/// by one of [`EC2_ID_HEX_LENGTHS`] lowercase hexadecimal characters.
pub fn is_ec2_resource_id(value: &str, prefix: &str) -> bool {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return false;
    };
    EC2_ID_HEX_LENGTHS.contains(&suffix.len())
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Refuse a list whose members are not EC2 resource ids of the type `prefix` names.
///
/// `SubnetIdList` and `SecurityGroupIdList` are lists of unconstrained `__string` in the
/// model (`smithy_query(query="shape", shape="SubnetIdList")`), so the id form is the
/// handler's to enforce rather than the boundary validator's: a value that is not an id
/// of that resource type names no resource the link could ever attach to. `Err` carries
/// the message for the operation's `BadRequestException`.
pub fn validate_ec2_resource_ids(
    member: &str,
    prefix: &str,
    ids: &[String],
) -> Result<(), String> {
    for id in ids {
        if !is_ec2_resource_id(id, prefix) {
            return Err(format!(
                "Invalid value for {member}: '{id}'. Each value must be of the form \
                 {prefix}<8 or 17 hexadecimal characters>."
            ));
        }
    }
    Ok(())
}

/// The longest `Name` a VPC link accepts.
///
/// `CreateVpcLink$Name` and `UpdateVpcLink$Name` both target
/// `StringWithLengthBetween1And128`, whose own documentation is "A string with a length
/// between [1-128]" (`smithy_query(query="shape",
/// shape="StringWithLengthBetween1And128")`). The shape carries no `@length` trait, so
/// the bound is a handler's to enforce rather than the boundary validator's.
pub const MAX_VPC_LINK_NAME_LEN: usize = 128;

/// Refuse a VPC link `Name` outside the documented bound of the shape it targets: at
/// least one character and at most [`MAX_VPC_LINK_NAME_LEN`].
///
/// `Ok(())` or the message for the operation's `BadRequestException`: it names the member
/// and the bound it broke.
pub fn validate_vpc_link_name(name: &str) -> Result<(), String> {
    let length = name.chars().count();
    if length == 0 || length > MAX_VPC_LINK_NAME_LEN {
        return Err(format!(
            "Invalid value for Name: must have length between 1 and \
             {MAX_VPC_LINK_NAME_LEN}, got {length}."
        ));
    }
    Ok(())
}

/// The ARN of a portal: API Gateway's ARNs name the resource by its own REST path
/// (`/apis/{apiId}` for an api, `/domainnames/{name}` for a domain name), so a portal's
/// is `/portals/{portalId}`. Built through `engine::emulator_arn`, so the account
/// segment is the emulator's canonical one.
pub fn portal_arn(region: &str, portal_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("/portals/{portal_id}"))
}

/// The default domain name API Gateway generates for a portal: the host a consumer
/// reaches the portal on when the account brought no domain of its own
/// (`EndpointConfigurationResponse$PortalDefaultDomainName`: "This domain name is
/// generated and managed by API Gateway").
///
/// AWS publishes no format for it, so the host follows this service's own convention
/// for a generated endpoint host — `{resource-id}.{subdomain}.{region}.amazonaws.com`,
/// as [`execute_api_endpoint`] builds for an api. What the member has to hold is a
/// name that is unique to the portal, stable across reads and within the modelled
/// 3..256 bound; the subdomain label is CHOSEN.
pub fn portal_default_domain_name(portal_id: &str, region: &str) -> String {
    format!("{portal_id}.portal.{region}.amazonaws.com")
}

/// The URL a portal's preview is served on.
///
/// A preview is the portal's own content served before anything is published, so it is
/// served on the domain API Gateway generates and manages for the portal
/// ([`portal_default_domain_name`]) rather than on a domain the account brought, which
/// need not resolve to the portal yet, under a path of its own.
///
/// AWS publishes no format for `Preview$PreviewUrl`, so the path label is CHOSEN; what
/// the member has to hold is a URL that names THIS portal and is stable across reads,
/// which it is because it is derived from the endpoint configuration the create
/// resolved once.
pub fn portal_preview_url(endpoint: &wire::EndpointConfigurationResponse) -> String {
    format!("https://{}/preview", endpoint.portal_default_domain_name)
}

/// How many characters follow the leading `Z` of a hosted zone id, matching the
/// fourteen-character ids AWS publishes for its endpoint hosted zones.
const HOSTED_ZONE_ID_LEN: usize = 13;

/// The hosted zone id of the portal domain
/// (`EndpointConfigurationResponse$PortalDomainHostedZoneId`), the zone an alias record
/// for the portal's domain points at.
///
/// The member is REQUIRED on the modelled response, so a portal always reports one. AWS
/// assigns ONE opaque hosted zone id per service endpoint per Region and publishes no
/// formula for it, so this DERIVES a stable id per region rather than carrying a table of
/// real ones it could not keep true. The mixing is
/// [`::service_common::stable_hex_id`] — the tree's one derived-id mixer, reproducible
/// across builds — uppercased and cut to the published LENGTH: a leading `Z` and
/// [`HOSTED_ZONE_ID_LEN`] characters, which the mixer renders as uppercase hexadecimal
/// (a subset of the uppercase alphanumerics AWS's own ids use, e.g. `Z1UJRXOUMOOFQ8`). A
/// region therefore always answers the same id, and two regions answer different ones.
pub fn portal_domain_hosted_zone_id(region: &str) -> String {
    let mixed: String = ::service_common::stable_hex_id(&[region])
        .to_uppercase()
        .chars()
        .take(HOSTED_ZONE_ID_LEN)
        .collect();
    format!("Z{mixed}")
}

/// Resolve a portal's requested domain configuration into the one the readers report,
/// generating the default domain of a portal with this id in this Region.
///
/// The request names either a domain the account owns together with its ACM certificate
/// (`AcmManaged`) or API Gateway's default domain (`None`); either way the response
/// carries the generated default domain name and its hosted zone id, and reports
/// `DomainName`/`CertificateArn` only for the domain the account brought.
pub fn resolve_portal_endpoint_configuration(
    portal_id: &str,
    region: &str,
    requested: &wire::EndpointConfigurationRequest,
) -> wire::EndpointConfigurationResponse {
    portal_endpoint_configuration(
        portal_default_domain_name(portal_id, region),
        portal_domain_hosted_zone_id(region),
        requested,
    )
}

/// Resolve a requested domain configuration onto a default domain the service has
/// ALREADY generated for the portal.
///
/// Which domain the account brought is the request's to say, while the default domain
/// name and its hosted zone id are generated from the portal's id and the Region the
/// portal lives in — so a later write to a portal's domain configuration carries those
/// two over rather than rebuilding them from whatever Region the writing request was
/// signed for. [`resolve_portal_endpoint_configuration`] is this with the pair generated
/// for a given id and Region.
pub fn portal_endpoint_configuration(
    portal_default_domain_name: String,
    portal_domain_hosted_zone_id: String,
    requested: &wire::EndpointConfigurationRequest,
) -> wire::EndpointConfigurationResponse {
    wire::EndpointConfigurationResponse {
        certificate_arn: requested
            .acm_managed
            .as_ref()
            .map(|managed| managed.certificate_arn.clone()),
        domain_name: requested
            .acm_managed
            .as_ref()
            .map(|managed| managed.domain_name.clone()),
        portal_default_domain_name,
        portal_domain_hosted_zone_id,
    }
}

/// Refuse a `SubnetIds` list that no VPC link could be built from: one naming no subnet
/// at all, or one naming a value that is not a subnet id.
///
/// `SubnetIds` is required on both the request and the modelled `VpcLink` shape, and a
/// link's network interfaces live IN its subnets, so a link with none has nowhere to
/// attach one. `Err` carries the message for `BadRequestException`.
pub fn validate_subnet_ids(subnet_ids: &[String]) -> Result<(), String> {
    if subnet_ids.is_empty() {
        return Err("Invalid value for SubnetIds: a VPC link must specify at least one \
                    subnet."
            .to_string());
    }
    validate_ec2_resource_ids("SubnetIds", SUBNET_ID_PREFIX, subnet_ids)
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

/// Resolve a request's whole `DomainNameConfigurations` list into the records to store.
///
/// A custom domain name is reached only through the endpoints this list describes — each
/// entry names the certificate the endpoint presents, its endpoint type, its security
/// policy and its address family — so a list with no entry at all describes a domain name
/// nothing can serve. A request that would leave the resource in that shape is refused
/// instead of stored: `Err` carries the message for the operation's `BadRequestException`
/// ("the input is incomplete or incorrect"). Each entry is resolved by
/// [`resolve_domain_name_endpoint`], so a create and an update cannot drift.
pub fn resolve_domain_name_endpoints(
    requested: &[wire::DomainNameConfiguration],
) -> Result<Vec<DomainNameEndpoint>, String> {
    if requested.is_empty() {
        return Err(EMPTY_DOMAIN_NAME_CONFIGURATIONS.to_string());
    }
    requested
        .iter()
        .enumerate()
        .map(|(index, entry)| resolve_domain_name_endpoint(index, entry))
        .collect()
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
pub fn to_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
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

/// Read a boolean out of a query parameter the model types as `__string`.
///
/// A query parameter modelled as a string carries its boolean as text, so the parse is
/// the handler's: `true` and `false` in any case are the two values a boolean query
/// parameter spells, and anything else says neither. `member` names the parameter in the
/// message; `Err` carries the message for the operation's `BadRequestException`. A
/// parameter the request left out answers `false`, the behaviour of a flag nobody set.
pub fn parse_boolean_query(member: &str, raw: Option<&str>) -> Result<bool, String> {
    match raw {
        None => Ok(false),
        Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Some(value) => Err(format!(
            "Invalid value for {member}: '{value}'. {member} must be true or false."
        )),
    }
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
