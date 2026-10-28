// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` DATA plane: how a request addressed to a deployed REST API stage's
//! invoke URL is recognised and answered.
//!
//! A REST API with a deployment is reached at
//! `https://{rest-api-id}.execute-api.{region}.amazonaws.com/{stage}/{resource-path}` —
//! the endpoint the `RestApi$disableExecuteApiEndpoint` documentation names ("the default
//! `https://{api_id}.execute-api.{region}.amazonaws.com` endpoint"). That host is NOT the
//! control plane's (`apigateway.{region}.amazonaws.com`): its first label is the API's own
//! id, which names no service, so the emulator's front door can resolve no target service
//! from such a request and answers it `404 UnknownService` — for a URL this very service
//! told the caller to use. The one thing that is never correct is for the host to be
//! unknown to the service.
//!
//! So this module is the second half of [`crate::service::EmulatedService`]: the claim
//! ([`claims_parts`], read by `Service::can_handle`) and the serving
//! ([`serve`], reached from the two seams the router offers before and after
//! authentication). Every status it answers is computed from the control plane's own
//! records, and the one thing it WRITES is the metering a usage plan's throttle consists
//! of ([`metered_refusal`]): the rate a key's requests are admitted at cannot be computed
//! without recording the ones already admitted. Nothing else about a stage, an api or a
//! key changes because it was invoked.
//!
//! A client on the machine this emulator runs on cannot reach that host at all: nothing
//! resolves `{rest-api-id}.execute-api.{region}.amazonaws.com` there, so it points at the
//! emulator's own endpoint and carries in the PATH what it cannot carry in the host — this
//! data plane and the api id behind the reserved `/_aws/execute-api/{api-id}` prefix, the
//! api id ahead of the stage, or the stage alone. [`resolve`] reads all four addressings,
//! and each one is checked against this service's own records, so one request still has
//! one claimant.
//!
//! # What it resolves, in order
//!
//! 1. the request names a stored `RestApi` — by the host's first label, by the api id
//!    behind the data-plane path prefix, by the first path segment, or, on a host that names
//!    no service, by the stage segment of its path ([`resolve`]) — whose
//!    `disableExecuteApiEndpoint` must not have turned this endpoint off;
//! 2. the first segment of the path WITHIN that api names a `Stage` of it, and that
//!    stage's `deploymentId` names a stored `Deployment`;
//! 3. the rest of the path matches one of the resource paths THAT DEPLOYMENT holds
//!    (`Deployment$apiSummary`), most specific match first: a literal segment beats a
//!    `{parameter}`, which beats a greedy `{proxy+}`;
//! 4. the request's verb names a method of that resource in the deployment, or the
//!    `ANY` catch-all;
//! 5. the method's deployed `authorizationType` is enforced, and then the API's own
//!    RESOURCE POLICY is applied ([`super::resource_policy`]): `RestApi$policy` "applies to
//!    this RestApi regardless of the caller and Method configuration", so a document the
//!    developer attached decides every request this endpoint answers — a private API pinned
//!    to one of its interface VPC endpoints admits the request that arrived through that
//!    endpoint and refuses the one that did not;
//! 6. `apiKeyRequired` is enforced — a key-required method admits only an enabled API key
//!    the records associate with THIS stage, through a usage plan over that api stage or
//!    the key's own deprecated `stageKeys`, and refuses every other request before the
//!    integration is reached;
//! 7. the method's REQUEST VALIDATOR is applied — the `RequestValidator` the live method's
//!    `requestValidatorId` names decides whether the request's parameters
//!    ([`parameters_refusal`]) and its payload ([`PendingBodyCheck::refusal`]) are checked
//!    before the integration is reached;
//! 8. the usage plan metering the presented API key admits the request at the plan's
//!    throttle, or the request is shed ([`metered_refusal`]);
//! 9. the method's integration decides the response.
//!
//! A step that resolves nothing is answered the way API Gateway answers it: through the
//! API's GATEWAY RESPONSE for the matching response type ([`GatewayRefusal`]), so a
//! developer's `PutGatewayResponse` customization of that type — its status code and its
//! template — is what the caller sees, and the uncustomized case falls back to the status
//! code the response type documents ([`crate::state::default_gateway_status_code`]).
//!
//! Whatever the outcome, the response NAMES THE REQUEST it answered: every response this
//! endpoint serves carries a freshly minted per-request id in the header API Gateway
//! reports it in, which is what lets a client correlate the answer it holds with what the
//! API processed. [`serve`] is the one funnel both seams reach, so no outcome can answer
//! without one.
//!
//! # What it does not do, and what it answers instead
//!
//! Only the `MOCK` integration type returns a backend's answer without a backend: it is
//! "for testing without actually invoking the backend" (`IntegrationType`), so its
//! response is entirely the API's own records — the integration response the call's
//! request template selects, the response template for the negotiated content type, and
//! the headers the method response declares. That path is served in full, from state. One
//! narrowing is worth stating: the status the request template names selects the
//! integration response stored UNDER THAT STATUS CODE, and otherwise the default one — a
//! `selectionPattern` is not matched against it, because a pattern is documented as
//! choosing "an integration response based on the response from the back end" and a `MOCK`
//! produces no back-end response to match.
//!
//! An `HTTP_PROXY` integration — the type `IntegrationType` gives "for integrating with the
//! HTTP proxy integration" — is PASSED THROUGH rather than mapped: its answer is the
//! endpoint's own. Where its `uri` names an endpoint THIS EMULATOR serves, which is every
//! `execute-api` invoke endpoint of this account, that pass-through is performed IN PROCESS
//! ([`passed_through`]): the caller's own request is re-addressed at the endpoint the `uri`
//! names, carrying the headers `requestParameters` maps onto it, and handed back to
//! [`serve_resolved`] to be served by that endpoint's own records — including its api's
//! resource policy, which is how a REGIONAL api fronting a PRIVATE one through a VPC link is
//! gated on the interface endpoint the `uri` names. No socket is opened and no name is
//! resolved: an address outside this emulator's own endpoints is never dialled, it is
//! refused, and nothing is invented either — the response is the one this data plane
//! computes for the request it passed on, whatever that is. What the onward request does NOT
//! carry is the caller's own SigV4 credential, in either the header or the presigned-query
//! form ([`HEADERS_NOT_PASSED_THROUGH`]): a signature over the address the caller used
//! authenticates nothing at the address the request is passed on to. A private integration
//! is carried only over a VPC link this account holds that has finished provisioning
//! ([`unusable_vpc_link`]), the chain of hops is bounded ([`MAX_PASSED_THROUGH_HOPS`]), and a
//! `requestParameters` entry outside the mapping forms below is an integration this call
//! cannot perform rather than a mapping silently dropped.
//!
//! Every other integration type names a backend this emulator cannot call:
//!
//! * `AWS` / `AWS_PROXY` invoke a Lambda function or another service's action. Lambda's
//!   own `Invoke` answers `501` for `RequestResponse` and `Event` — this emulator has no
//!   in-process runtime for a deployment package (see
//!   `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`) — so no
//!   function's answer exists to map;
//! * `HTTP`, and an `HTTP_PROXY` whose `uri` names an endpoint outside this emulator,
//!   address an endpoint over the network, and this emulator makes no outbound connection.
//!   The standard `HTTP` integration is not served even against an endpoint of this
//!   emulator, because its response is chosen by matching the BACKEND's response against
//!   each integration response's `selectionPattern` and then rendered through that
//!   response's templates — a mapping half a proxy integration has none of.
//!
//! None of those is faked: a body, a status or a header invented for a backend that never
//! ran would be indistinguishable from a real one. They are answered as API Gateway
//! answers an integration it could not perform — the `API_CONFIGURATION_ERROR` gateway
//! response, `500` with `{"message":"Internal server error"}` — and the reason is logged.
//! The same goes for a mapping template that is a Velocity TEMPLATE rather than a literal
//! body: evaluating Velocity is not something this emulator does, and a hand-written
//! subset of it would silently produce the wrong body, so such a template is an
//! integration this call cannot perform rather than a body built by guesswork.
//!
//! # The half `TestInvokeMethod` shares
//!
//! `TestInvokeMethod` simulates "the invocation of a Method in your RestApi" — the same
//! integration, read out of the same records, for a request that never reached a socket. So
//! the performing half of this module takes no wire request: [`perform_integration`] reads
//! what an integration needs through [`MethodRequest`], and [`refusal_rendering`] gives the
//! status and body a call no integration can serve is answered with. That operation REPORTS
//! what this module performs FROM THE API'S OWN RECORDS, so neither can answer one stored
//! configuration differently. Passing a request through is not that: the answer is another
//! endpoint's response to a request this one forwarded, and a simulated invocation forwards
//! nothing, so it keeps reporting such an integration as a configuration error.

use std::borrow::Cow;
use std::collections::BTreeMap;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::http;

use crate::ops::put_integration::{
    DEFAULT_CONNECTION_TYPE, HTTP_PROXY_INTEGRATION_TYPE, MOCK_INTEGRATION_TYPE,
    VPC_LINK_CONNECTION_TYPE,
};
use crate::ops::put_integration_response::is_static_mapping_value;
use crate::ops::put_method::method_key;
use crate::ops::put_method_response::METHOD_RESPONSE_HEADER_PREFIX;
use crate::ops::types::{GetIntegrationOutput, GetIntegrationResponseOutput};
use crate::state::{ApiKeyEntry, DeploymentEntry, MethodEntry, RestApiEntry};

use super::resource_policy;

/// The second label of every `execute-api` endpoint host: `{api-id}.execute-api.…`.
///
/// From `RestApi$disableExecuteApiEndpoint`: "clients can invoke your API with the default
/// `https://{api_id}.execute-api.{region}.amazonaws.com` endpoint".
pub const EXECUTE_API_LABEL: &str = "execute-api";

/// The reserved path segment that names a DATA plane on the one endpoint this emulator
/// serves its whole surface at, ahead of [`EXECUTE_API_LABEL`] and the api id:
/// `/_aws/execute-api/{api-id}/{stage}/{resource-path}`.
///
/// A client reaching AWS addresses the api by host; a client reaching this emulator has
/// one host for every service and every plane, so the two parts of an invoke URL that
/// AWS puts in the host — that it is the `execute-api` data plane, and which api — travel
/// in the path instead. The segment begins with `_`, which no modelled path segment of any
/// service does, so the prefix cannot collide with a control-plane route however a stage or
/// a resource is named.
const EMULATOR_PATH_NAMESPACE: &str = "_aws";

/// The path segment that belongs to ApiGatewayManagementApi rather than to a stage.
///
/// `@connections` is bound on the SAME `execute-api` host — that service's documentation
/// gives its endpoint as `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}` and
/// binds `DELETE|GET|POST /@connections/{ConnectionId}` under it — so a request naming it
/// is not a stage invoke and is left to the service that models it. Claiming it here would
/// make two services claim one request, which the router rejects as `AmbiguousService`.
const CONNECTIONS_SEGMENT: &str = "@connections";

/// The prefix of every VPC endpoint id, which is also the first label of the
/// endpoint-specific DNS names an interface endpoint answers on.
///
/// An interface endpoint to the Region's `execute-api` service is given endpoint-specific
/// names of the form `{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com` (and one
/// `{vpc-endpoint-id}-{zone}.…` per Availability Zone), which is how a client addresses a
/// PRIVATE api through one endpoint rather than another — see [`endpoint_of_host`].
const VPC_ENDPOINT_ID_PREFIX: &str = "vpce-";

/// The lengths of the hexadecimal part of a VPC endpoint id, from EC2's own id pattern
/// (`^vpce-([0-9a-f]{8}|[0-9a-f]{17})$`): the two widths AWS mints.
const VPC_ENDPOINT_ID_HEX_LENGTHS: &[usize] = &[8, 17];

/// The header a client names the api in when it addresses a PRIVATE api by its VPC
/// endpoint's own DNS name, which names the endpoint rather than the api: AWS documents
/// invoking such an api through the endpoint-specific hostname with the api id carried in
/// `x-apigw-api-id`.
const API_ID_HEADER: &str = "x-apigw-api-id";

/// The verb-independent catch-all a method may be put under, from
/// [`crate::ops::put_method::HTTP_METHODS`]: "plus the `ANY` catch-all that matches every
/// verb".
const ANY_METHOD: &str = "ANY";

/// The `authorizationType` that lets a request through with no authorization at all. From
/// `Method$authorizationType`: "the method's authorization type. Valid values are `NONE`
/// for open access …".
const AUTHORIZATION_NONE: &str = "NONE";

/// The `authorizationType` whose caller must sign the request: "`AWS_IAM` for using AWS
/// IAM permissions" (`Method$authorizationType`).
const AUTHORIZATION_AWS_IAM: &str = "AWS_IAM";

/// The `authorizationType`s served by an `Authorizer` of the API: "`CUSTOM` for using a
/// custom authorizer, or `COGNITO_USER_POOLS` for using a Cognito user pool"
/// (`Method$authorizationType`).
const AUTHORIZATION_BY_AUTHORIZER: &[&str] = &["CUSTOM", "COGNITO_USER_POOLS"];

/// The header an API key travels in when the API's `apiKeySource` is `HEADER`, which is
/// what that member documents: "`HEADER` to read the API key from the `X-API-Key` header of
/// a request".
const API_KEY_HEADER: &str = "x-api-key";

/// The `apiKeySource` that reads the key from the request header; see [`API_KEY_HEADER`].
const API_KEY_SOURCE_HEADER: &str = "HEADER";

/// The mapping-expression prefix of a request HEADER, which is how both an authorizer's
/// identity source and a method's request parameter name one.
///
/// From `Method$requestParameters`: "a key is a method request parameter name matching the
/// pattern of `method.request.{location}.{name}`, where `location` is `querystring`,
/// `path`, or `header`"; `Authorizer$identitySource` names a header the same way ("the
/// request header mapping expression for the custom header holding the authorization token
/// … `method.request.header.Auth`").
const METHOD_REQUEST_HEADER_PREFIX: &str = "method.request.header.";

/// The mapping-expression prefix of a query-string parameter; see
/// [`METHOD_REQUEST_HEADER_PREFIX`] for the pattern both members share.
const METHOD_REQUEST_QUERY_PREFIX: &str = "method.request.querystring.";

/// The mapping-expression prefix of a PATH parameter, the third `location`
/// [`METHOD_REQUEST_HEADER_PREFIX`] quotes.
const METHOD_REQUEST_PATH_PREFIX: &str = "method.request.path.";

/// The mapping-expression prefix of a query-string parameter taken with every one of its
/// values, the multi-value spelling of [`METHOD_REQUEST_QUERY_PREFIX`].
const METHOD_REQUEST_MULTI_QUERY_PREFIX: &str = "method.request.multivaluequerystring.";

/// The multi-value spelling of [`METHOD_REQUEST_HEADER_PREFIX`].
const METHOD_REQUEST_MULTI_HEADER_PREFIX: &str = "method.request.multivalueheader.";

/// The mapping-expression prefix of a header of the INTEGRATION request — the request the
/// backend receives — which is how `Integration$requestParameters` names one it is to carry:
/// "The key is an integration request parameter name", written
/// `integration.request.{location}.{name}` over the same three locations a method request
/// parameter is written over.
const INTEGRATION_REQUEST_HEADER_PREFIX: &str = "integration.request.header.";

/// The query parameters a PRESIGNED SigV4 credential is carried in — the query-string form
/// of the headers [`HEADERS_NOT_PASSED_THROUGH`] withholds, and the set AWS documents a
/// presigned request carries (`X-Amz-Algorithm`, `X-Amz-Credential`, `X-Amz-Date`,
/// `X-Amz-Expires`, `X-Amz-SignedHeaders`, `X-Amz-Signature`, `X-Amz-Security-Token`,
/// `X-Amz-Content-Sha256`). Compared lowercased; every OTHER query parameter the caller
/// sent travels on, whatever it is named.
const SIGNED_QUERY_PARAMETERS: &[&str] = &[
    "x-amz-algorithm",
    "x-amz-credential",
    "x-amz-date",
    "x-amz-expires",
    "x-amz-signedheaders",
    "x-amz-signature",
    "x-amz-security-token",
    "x-amz-content-sha256",
];

/// How many passed-through integrations one invoke may follow.
///
/// A proxy integration whose `uri` names an endpoint of this emulator may reach an api whose
/// own method passes the request on again, so the chain is bounded: a configuration that
/// leads back to itself is answered through the api's `API_CONFIGURATION_ERROR` gateway
/// response rather than followed until the stack is gone. API Gateway publishes no hop
/// count, so this is the emulator's own bound and is deliberately small — a front api in
/// front of a private one is one hop.
const MAX_PASSED_THROUGH_HOPS: u8 = 4;

/// The request headers a passed-through request does NOT carry over from the caller's.
///
/// `host` addresses the endpoint, and the endpoint the request is passed on to is the one
/// the integration's `uri` names, not the one the caller addressed; `content-length` is
/// recomputed for the body as it is sent.
///
/// The SigV4 authentication headers are not carried either, and that is the substantive one.
/// A caller's signature is computed over the endpoint, the path and the headers of the
/// request the caller made, so at the endpoint the request is passed on to it authenticates
/// nothing: carrying it would make an onward request LOOK credentialed to a method whose
/// `authorizationType` is `AWS_IAM` on the strength of a signature over somebody else's
/// address. API Gateway's own integration credentials are `Integration$credentials` — "the
/// credentials required for the integration" — which is a record of the integration rather
/// than anything the caller sent. Everything else the caller sent travels on, which is what
/// "passed through" means.
const HEADERS_NOT_PASSED_THROUGH: &[&str] = &[
    "host",
    "content-length",
    "authorization",
    "x-amz-security-token",
    "x-amz-date",
    "x-amz-content-sha256",
];

/// The media type a gateway response and an untemplated integration response are rendered
/// with. API Gateway's own default gateway-response templates are
/// `application/json`-keyed, and a `MOCK` integration with no response template answers an
/// empty JSON body.
///
/// `pub` because a caller that REPORTS a rendering instead of sending it
/// (`TestInvokeMethod`) has to report the media type it was rendered with.
pub const JSON_CONTENT_TYPE: &str = "application/json";

/// The `passthroughBehavior` that passes an unmapped body through whenever no template
/// keys the request's content type; the default one
/// ([`crate::ops::put_integration::DEFAULT_PASSTHROUGH_BEHAVIOR`]).
const PASSTHROUGH_WHEN_NO_MATCH: &str = "WHEN_NO_MATCH";

/// The `passthroughBehavior` that passes no unmapped body through at all, which
/// `PutIntegrationRequest$passthroughBehavior` documents as rejecting the request "with an
/// HTTP 415 Unsupported Media Type response".
const PASSTHROUGH_NEVER: &str = "NEVER";

/// The member a gateway response body carries the reason under: API Gateway's default
/// gateway-response template is `{"message":$context.error.messageString}`.
const MESSAGE_MEMBER: &str = "message";

/// The member an AUTHORIZATION refusal's body carries its sentence under.
///
/// The `execute-api` endpoint answers a request its api's resource policy does not allow
/// with the IAM-shaped body — `{"Message":"User: … is not authorized to perform: …"}` — so
/// a client reading that sentence reads it from this member rather than from the
/// [`MESSAGE_MEMBER`] every other gateway response carries.
const AUTHORIZATION_MESSAGE_MEMBER: &str = "Message";

/// The status code a refusal falls back to when its response type names none and no
/// customization overrides it. `DEFAULT_5XX`'s own class: this emulator could not place
/// the refusal, which is a server-side fault, never a caller's.
const UNPLACEABLE_REFUSAL_STATUS: u16 = 500;

/// The gateway response type whose customization applies to any 4xx the API has no
/// specific customization for; from [`crate::state`]'s own note on the two rows that carry
/// no status code of their own, "each is a fallback for a whole class of responses".
const DEFAULT_4XX: &str = "DEFAULT_4XX";

/// The gateway response type whose customization applies to any 5xx; see [`DEFAULT_4XX`].
const DEFAULT_5XX: &str = "DEFAULT_5XX";

/// One refusal of the data plane: the GATEWAY RESPONSE TYPE it is answered through — which
/// decides its status code, and which customization of the API applies to it — and the
/// sentence API Gateway's own default body carries for it.
///
/// The type is the unit of customization (`PutGatewayResponse` takes a `responseType`), so
/// naming it here is what lets a developer's customization reach the caller without this
/// module knowing anything about it.
#[derive(Debug, Clone)]
pub struct GatewayRefusal {
    /// A value of the model's `GatewayResponseType` enum.
    response_type: &'static str,
    /// The sentence the uncustomized response body carries. Owned-or-borrowed because a
    /// few refusals state what the request was missing, which is read off the request
    /// rather than fixed in the binary.
    message: Cow<'static, str>,
    /// The error shape this refusal is the wire form of, reported in
    /// [`ERROR_TYPE_HEADER`], or `None` for a response type whose exception name this
    /// service's model does not declare.
    error_type: Option<&'static str>,
    /// What a VALIDATION of the request found, as the gateway response's
    /// `$context.error.validationErrorString` reports it; empty for every refusal that is
    /// not the outcome of validating the request against what the method declares.
    validation_detail: Cow<'static, str>,
    /// The member API Gateway's own default body carries the sentence under. Every gateway
    /// response's own default template is `{"message":$context.error.messageString}`, so
    /// that is [`MESSAGE_MEMBER`] for all but the AUTHORIZATION refusal, which API Gateway
    /// answers with the IAM-shaped [`AUTHORIZATION_MESSAGE_MEMBER`] body.
    message_member: &'static str,
}

impl GatewayRefusal {
    /// The `GatewayResponseType` this refusal is answered through — the one a developer's
    /// `PutGatewayResponse` customization of it would apply to. Read by the caller that
    /// REPORTS a refusal rather than answering it on the wire (`TestInvokeMethod`'s
    /// execution log), so the report names the same gateway response the data plane uses.
    pub fn response_type(&self) -> &'static str {
        self.response_type
    }
}

/// The API, the stage or the endpoint does not serve this request at all: API Gateway
/// answers `403` with `{"message":"Forbidden"}` for an API id it does not serve, a stage
/// that has no deployment, and an endpoint a developer has turned off.
const FORBIDDEN: GatewayRefusal = GatewayRefusal {
    response_type: "ACCESS_DENIED",
    message: Cow::Borrowed("Forbidden"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// Nothing in the DEPLOYMENT matches the path and verb, or the method's authorization
/// needs a credential the request did not carry: both are
/// `{"message":"Missing Authentication Token"}` with `403`, which is what API Gateway
/// answers for a route it cannot place and for an `AWS_IAM` method reached unsigned.
const MISSING_AUTHENTICATION_TOKEN: GatewayRefusal = GatewayRefusal {
    response_type: "MISSING_AUTHENTICATION_TOKEN",
    message: Cow::Borrowed("Missing Authentication Token"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method requires an API key and the request presented none this API's records
/// associate with this stage: `403` `{"message":"Forbidden"}` through `INVALID_API_KEY`.
const INVALID_API_KEY: GatewayRefusal = GatewayRefusal {
    response_type: "INVALID_API_KEY",
    message: Cow::Borrowed("Forbidden"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// An authorizer's identity source is absent from the request, so the authorizer is not
/// even called: `401` `{"message":"Unauthorized"}`.
const UNAUTHORIZED: GatewayRefusal = GatewayRefusal {
    response_type: "UNAUTHORIZED",
    message: Cow::Borrowed("Unauthorized"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method's configuration cannot serve the call: no integration on the deployed
/// method, an integration whose type names a backend this emulator cannot call, or a
/// mapping template it cannot evaluate. `500` `{"message":"Internal server error"}`.
const API_CONFIGURATION_ERROR: GatewayRefusal = GatewayRefusal {
    response_type: "API_CONFIGURATION_ERROR",
    message: Cow::Borrowed("Internal server error"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method is authorized by an `Authorizer` whose decision this emulator cannot
/// obtain — the authorizer runs a Lambda function, and no deployment package runs here.
/// `500` `{"message":"Internal server error"}`, the same class API Gateway reports when it
/// cannot invoke the authorizer.
const AUTHORIZER_CONFIGURATION_ERROR: GatewayRefusal = GatewayRefusal {
    response_type: "AUTHORIZER_CONFIGURATION_ERROR",
    message: Cow::Borrowed("Internal server error"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The request's content type maps to no integration request template and the
/// integration's `passthroughBehavior` does not let an unmapped body through: `415`
/// `{"message":"Unsupported Media Type"}`.
const UNSUPPORTED_MEDIA_TYPE: GatewayRefusal = GatewayRefusal {
    response_type: "UNSUPPORTED_MEDIA_TYPE",
    message: Cow::Borrowed("Unsupported Media Type"),
    error_type: None,
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method's request validator declares `validateRequestBody` and the request's payload
/// does not satisfy the JSON Schema of the `Model` the method maps its content type to:
/// `400` `{"message":"Invalid request body"}` through `BAD_REQUEST_BODY`.
///
/// The sentence is FIXED — it names neither the model nor what about the payload failed.
/// WHAT failed is the validation's own report, which a gateway response template reads
/// through `$context.error.validationErrorString`: `detail` is that report, and it is the
/// validator's own finding about THIS payload rather than a sentence written here.
fn bad_request_body(detail: String) -> GatewayRefusal {
    GatewayRefusal {
        response_type: "BAD_REQUEST_BODY",
        message: Cow::Borrowed("Invalid request body"),
        error_type: Some("BadRequestException"),
        validation_detail: Cow::Owned(detail),
        message_member: MESSAGE_MEMBER,
    }
}

/// The usage plan metering this request's API key admits no further request at this
/// instant: `429` `{"message":"Too Many Requests"}` through `THROTTLED`. The plan's
/// numeric limits are what decide the refusal, not what it says.
const THROTTLED: GatewayRefusal = GatewayRefusal {
    response_type: "THROTTLED",
    message: Cow::Borrowed("Too Many Requests"),
    error_type: Some("TooManyRequestsException"),
    validation_detail: Cow::Borrowed(""),
    message_member: MESSAGE_MEMBER,
};

/// The method's request validator declares `validateRequestParameters` and the request
/// does not carry every request parameter the method declares REQUIRED: `400` through
/// `BAD_REQUEST_PARAMETERS`.
///
/// The sentence's fixed part states the condition and its variable part is the list of
/// parameter NAMES the request was missing, read off the method's own
/// `requestParameters` declarations — which is what tells the caller what to add, and
/// which is also the validation's whole finding, so it is reported as the detail too.
fn bad_request_parameters(missing: &[String]) -> GatewayRefusal {
    let named = missing.join(", ");
    GatewayRefusal {
        response_type: "BAD_REQUEST_PARAMETERS",
        message: Cow::Owned(format!("Missing required request parameters: [{named}]")),
        error_type: Some("BadRequestException"),
        validation_detail: Cow::Owned(format!("[{named}]")),
        message_member: MESSAGE_MEMBER,
    }
}

/// The api's own RESOURCE POLICY does not let this request invoke the method: `403`,
/// carrying the sentence AWS refuses an unauthorized invoke with
/// ([`resource_policy::denial`]).
///
/// Answered through `ACCESS_DENIED`, the gateway response type an authorization failure is
/// customized under, so a developer's `PutGatewayResponse` for it reaches this refusal too.
/// The body's member is the IAM-shaped [`AUTHORIZATION_MESSAGE_MEMBER`] rather than the
/// [`MESSAGE_MEMBER`] every other gateway response carries, because that is the body API
/// Gateway answers an authorization refusal with.
fn access_denied(message: String) -> GatewayRefusal {
    GatewayRefusal {
        response_type: "ACCESS_DENIED",
        message: Cow::Owned(message),
        error_type: None,
        validation_detail: Cow::Borrowed(""),
        message_member: AUTHORIZATION_MESSAGE_MEMBER,
    }
}

/// The header API Gateway names the error shape of a refused request in, which is what an
/// SDK reads a restJson1 error code out of.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// What an invoke addresses, as the wire carries it: the request's host — absent on a
/// request that carries neither a `Host` header nor an absolute URI — its path, and the api
/// id a request addressed to a VPC endpoint's own DNS name carries in a header.
///
/// Built from BOTH shapes the router hands this module, the buffered parts the claim reads
/// and the runtime-API request the two serving seams read, so one rule resolves both and
/// the claim cannot resolve a different api than the answer does.
#[derive(Debug)]
pub struct Addressed<'a> {
    host: Option<&'a str>,
    path: &'a str,
    /// The [`API_ID_HEADER`] the request carries, which is how a request on an endpoint's
    /// own DNS name — a host that names the ENDPOINT, not the api — names the api.
    api_id_header: Option<&'a str>,
}

impl<'a> Addressed<'a> {
    /// The addressing of the buffered request these parts came from.
    fn of_parts(parts: &'a http::request::Parts) -> Self {
        Self {
            host: request_host(parts),
            path: parts.uri.path(),
            api_id_header: parts
                .headers
                .get(API_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
        }
    }

    /// The addressing of a runtime-API request, which carries its URI as a string.
    fn of_request(req: &'a RtRequest) -> Self {
        Self {
            host: req.headers().get("host").or_else(|| authority_of(req.uri())),
            path: path_of(req.uri()),
            api_id_header: req.headers().get(API_ID_HEADER),
        }
    }
}

/// What an invoke request addresses, as this service's records resolve it: the api, the
/// path WITHIN that api's endpoint — `/{stage}/{resource-path}`, which is what the stage
/// and the resource are read from — and the VPC endpoint the request arrived through, when
/// it was addressed on one's own DNS name.
#[derive(Debug)]
struct Resolved<'s, 'r> {
    api: &'s RestApiEntry,
    invoke_path: &'r str,
    /// The id of the interface VPC endpoint whose endpoint-specific DNS name the request
    /// was addressed on, or `None` on every other addressing — where the request names no
    /// endpoint and this service claims none for it.
    source_vpce: Option<&'r str>,
}

/// What `addressed` addresses, or `None` when this data plane serves it not at all.
///
/// FIVE addressings resolve, because a client reaches this emulator at a different
/// address than it reaches AWS at:
///
/// * HOST-STYLE, the addressing `RestApi$disableExecuteApiEndpoint` documents: the host's
///   first label names the api (`{api_id}.execute-api.{region}.amazonaws.com`) and the
///   whole path is the invoke path. A host that IS an `execute-api` endpoint but whose api
///   id this service does not hold resolves to nothing at all: that host is another api's
///   (an API Gateway V2 api is reached on the same host shape, and its own service claims
///   it), so reading its path as one of THIS service's stages would make two services claim
///   one request;
/// * VPC-ENDPOINT HOST-STYLE, the addressing a PRIVATE api is reached by through one of its
///   interface VPC endpoints: the host is that ENDPOINT's own DNS name
///   (`{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com`, the name EC2 reports in
///   the endpoint's `DnsEntries`), which names no api at all, so the api travels in the
///   [`API_ID_HEADER`]. This is the one addressing that tells this service WHICH endpoint a
///   request arrived through, which is what the api's resource policy can then be
///   conditioned on ([`super::resource_policy::SOURCE_VPCE_KEY`]);
/// * PREFIXED PATH-STYLE, the addressing a client reaching an EMULATOR uses: the data
///   plane is named by the reserved namespace segment [`EMULATOR_PATH_NAMESPACE`] and the
///   [`EXECUTE_API_LABEL`], then the api id, then the invoke path —
///   `/_aws/execute-api/{api_id}/{stage}/{resource-path}`. One
///   endpoint serves this emulator's whole surface, control plane and data plane alike, so
///   a client that cannot reach `{api_id}.execute-api.{region}.amazonaws.com` names the
///   data plane in the path before naming the api, which is what keeps an invoke path off
///   the control plane's own modelled path space whatever a stage is called;
/// * PATH-STYLE: the api id is the FIRST PATH SEGMENT and the invoke path is what follows
///   it. A client on the machine this emulator runs on resolves
///   `{api_id}.execute-api.{region}.amazonaws.com` nowhere, so it reaches the emulator's own
///   endpoint and carries in the path what it cannot carry in the host — the same move S3
///   path-style addressing makes with a bucket, and the same one Lightsail's container-service
///   URL makes with the labels AWS puts in a hostname. It is read only on a host that names
///   NO SERVICE (the emulator's own endpoint) or names THIS one, so a first segment that
///   happens to equal an api id cannot take a request away from the service whose endpoint
///   the host names;
/// * STAGE-ONLY, for a client that carries the invoke path alone: a host that names NO
///   SERVICE at all is resolved by the STAGE segment of the path, the only part of such a
///   request that names a deployed stage. That is the host-independent claim
///   ApiGatewayManagementApi makes over its own `@connections` path space and
///   `service-com-amazonaws-apigatewayv2-apigatewayv2`'s WebSocket data plane makes over a
///   stage's. Which service a host names is the FRONT DOOR's own rule
///   ([`service_named_by_host`]), so the stage is read exactly for the request the front door
///   could place nowhere, and a host that names some service — this service's own control
///   plane, or another service's endpoint — keeps answering as that service. The host alone
///   decides it, and not the whole routing key: an `AWS_IAM` method is invoked with a SigV4
///   credential, whose scope resolves a routing key for every such request, so a claim
///   conditioned on the key having resolved nothing would leave exactly the signed invokes
///   unclaimed. When several apis publish a stage of that name the request names no ONE
///   api, so nothing is resolved rather than one of them guessed at; each is reachable by an
///   addressing that names it, which is what the prefixed form above is for.
///
/// This service's own CONTROL-plane path space is never read as a stage: a request whose
/// first segment is one of the model's route segments is a control-plane call on whatever
/// host it arrives at, and answering it from the data plane would take `/restapis` away
/// from the operations that serve it. A stage named like one of those segments is reachable
/// through the three addressings above, where the api is named before the stage is.
fn resolve<'s, 'r>(
    store: &'s crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    addressed: &Addressed<'r>,
) -> Option<Resolved<'s, 'r>> {
    // The `@connections` path space belongs to ApiGatewayManagementApi, on either host —
    // see [`CONNECTIONS_SEGMENT`].
    if addresses_connections(addressed.path) {
        return None;
    }
    // VPC-ENDPOINT HOST-STYLE first: such a host IS an `execute-api` endpoint host, so the
    // api-id reading below would take its endpoint id for an api id and resolve nothing.
    if let Some(endpoint) = addressed.host.and_then(endpoint_of_host) {
        return addressed
            .api_id_header
            .and_then(|api_id| store.rest_apis.get(api_id))
            .map(|api| Resolved {
                api,
                invoke_path: addressed.path,
                source_vpce: Some(endpoint),
            });
    }
    if let Some(api_id) = addressed.host.and_then(api_id_of_host) {
        return store.rest_apis.get(api_id).map(|api| Resolved {
            api,
            invoke_path: addressed.path,
            source_vpce: None,
        });
    }
    // PREFIXED PATH-STYLE: the path names this data plane before it names the api, so it is
    // read on whatever host it arrives at — the prefix is in no service's modelled path
    // space, and an api id this service does not hold is served by nothing here.
    if let Some((api_id, invoke_path)) = split_execute_api_prefix(addressed.path) {
        return store.rest_apis.get(api_id).map(|api| Resolved {
            api,
            invoke_path,
            source_vpce: None,
        });
    }
    let (first, rest) = split_first_segment(addressed.path)?;
    // Which service the host names, by the front door's own rule: `None` is the emulator's
    // own endpoint, and this service's own key is its control-plane host.
    let host_service = addressed.host.and_then(service_named_by_host);
    if host_service
        .as_deref()
        .is_some_and(|service| service != crate::ROUTE_KEY)
    {
        // Another service's endpoint: whatever its path carries is that service's to
        // answer, so neither addressing below is read and one request keeps one claimant.
        return None;
    }
    // PATH-STYLE: the first segment names an api this service holds, and the invoke path
    // is what follows it.
    if let Some(api) = store.rest_apis.get(first) {
        return Some(Resolved {
            api,
            invoke_path: rest,
            source_vpce: None,
        });
    }
    if host_service.is_some() {
        // This service's own control-plane host: a path segment that is no api id of this
        // account is a control-plane call, not a stage.
        return None;
    }
    if crate::ROUTE_PATH_SEGMENTS.contains(&first) {
        return None;
    }
    // STAGE-ONLY: the first segment names a stage, and the api is the one that holds a
    // stage of that name. When several hold one the request names no ONE api, so nothing is
    // resolved rather than one of them guessed at — the three addressings above carry the
    // api id and are never ambiguous.
    let mut holding = store
        .rest_apis
        .values()
        .filter(|api| api.stages.contains_key(first));
    let api = holding.next()?;
    holding.next().is_none().then_some(Resolved {
        api,
        invoke_path: addressed.path,
        source_vpce: None,
    })
}

/// The api id and the invoke path of a PREFIXED PATH-STYLE address, or `None` when `path`
/// does not carry that prefix.
///
/// The prefix is [`EMULATOR_PATH_NAMESPACE`] then [`EXECUTE_API_LABEL`], by exact segment
/// equality, and what follows is the api id and then the invoke path the host-style
/// addressing carries whole — `/_aws/execute-api/{api-id}/{stage}/{resource-path}`. The
/// invoke path is a SLICE of the request's own path, so nothing is copied, and a prefix
/// with no api id after it resolves nothing.
fn split_execute_api_prefix(path: &str) -> Option<(&str, &str)> {
    let (prefix, after_prefix) = split_first_segment(path)?;
    if prefix != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (label, after_label) = split_first_segment(after_prefix)?;
    if label != EXECUTE_API_LABEL {
        return None;
    }
    split_first_segment(after_label)
}

/// The first non-empty segment of `path` and the path that follows it, or `None` for a
/// path with no segment at all.
///
/// The remainder keeps no leading `/` — [`path_segments`] drops empty segments, so the two
/// spellings read the same — and is a SLICE of the path, so a resolved addressing borrows
/// the request rather than copying it.
fn split_first_segment(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_start_matches('/');
    let (first, rest) = match trimmed.split_once('/') {
        Some((first, rest)) => (first, rest),
        None => (trimmed, ""),
    };
    (!first.is_empty()).then_some((first, rest))
}

/// The service `host` names to the emulator's front door, or `None` when it names none.
///
/// The front door's OWN host reader decides it (`emulator_server::service_from_host`, the
/// last tier of the routing key every front door resolves), rather than a second rule
/// written here: the request the two path addressings are read for is exactly the one that
/// reader can place nowhere — a loopback address or `localhost`, which is the only endpoint
/// a client on this machine can reach. It is handed the host as a `Host` header because
/// that is the shape it reads, and a host no header can carry names no service.
fn service_named_by_host(host: &str) -> Option<String> {
    let mut headers = http::HeaderMap::new();
    let value = http::HeaderValue::from_str(host).ok()?;
    headers.insert(http::header::HOST, value);
    ::emulator_server::service_from_host(&headers)
}

/// Whether THIS data plane serves the request these parts came from — the data-plane half
/// of `Service::can_handle`.
///
/// Narrowed to an api this service HOLDS, read from the one shared store, so nothing is
/// claimed that this service cannot answer about and exactly one service claims a request.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &http::request::Parts,
) -> bool {
    claims(engine, &Addressed::of_parts(parts))
}

/// [`claims_parts`] for the runtime-API request the two serving seams are handed.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    claims(engine, &Addressed::of_request(req))
}

/// Whether this data plane serves `addressed`.
fn claims<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> bool {
    crate::state::service_state(engine)
        .is_some_and(|state| resolve(&state.lock(), addressed).is_some())
}

/// The first label of `host` when `host` is an `execute-api` endpoint: the API id.
///
/// The shape is `{api-id}.execute-api.{region}.amazonaws.com` — a first label, then the
/// [`EXECUTE_API_LABEL`] label, then at least a region and a domain ([`endpoint_labels`]).
fn api_id_of_host(host: &str) -> Option<&str> {
    endpoint_labels(host).map(|(api_id, _region)| api_id)
}

/// The two parts every `execute-api` endpoint host carries: its FIRST label — which names
/// the api on an api's own endpoint and the VPC endpoint on an endpoint-specific one — and
/// the Region label that follows the [`EXECUTE_API_LABEL`].
///
/// One reader for both forms, because both put the same two parts in the same places:
/// `{api-id}.execute-api.{region}.amazonaws.com` and
/// `{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com`. Any `:port` and userinfo is
/// removed first, as the front door's own host reader does. The region and the domain are
/// not matched against a list: an emulator serves whatever region a caller addresses, and a
/// FIPS or a dualstack spelling of the same endpoint is the same endpoint.
fn endpoint_labels(host: &str) -> Option<(&str, &str)> {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    let without_port = without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo);
    let mut labels = without_port.split('.');
    let first = labels.next()?;
    let second = labels.next()?;
    // The label is `execute-api` itself, or one of its endpoint VARIANTS, which AWS spells
    // with a suffix after a hyphen (`execute-api-fips`). A label that merely STARTS with
    // those characters (`execute-apifoo`) is a different host and is not claimed.
    let execute_api = second == EXECUTE_API_LABEL
        || second
            .strip_prefix(EXECUTE_API_LABEL)
            .is_some_and(|variant| variant.starts_with('-') && variant.len() > 1);
    if first.is_empty() || !execute_api {
        return None;
    }
    // A region label and a domain: `{api-id}.execute-api.{region}.amazonaws.com` has four
    // labels after the id, and the shortest form this accepts still has two, so a bare
    // `execute-api.something` (no api id in front) is not read as one.
    let region = labels.next()?;
    (labels.count() >= 1).then_some((first, region))
}

/// The id of the interface VPC endpoint whose own DNS name `host` is, or `None` for a host
/// that is not one.
///
/// An interface endpoint to the Region's `execute-api` service answers on the
/// endpoint-specific names EC2 reports in its `DnsEntries` — the regional
/// `{vpc-endpoint-id}.execute-api.{region}.vpce.amazonaws.com` and one zonal
/// `{vpc-endpoint-id}-{zone}.…` per Availability Zone its interfaces sit in — so the id is
/// the first label up to the zone suffix, and the zonal and regional names resolve to the
/// SAME endpoint. The id is [`VPC_ENDPOINT_ID_PREFIX`] and one of the hexadecimal widths
/// EC2's id pattern admits ([`VPC_ENDPOINT_ID_HEX_LENGTHS`]), which is also what tells an
/// endpoint's own name apart from an api id that merely begins with those characters.
fn endpoint_of_host(host: &str) -> Option<&str> {
    let (first, _region) = endpoint_labels(host)?;
    let hex = first
        .strip_prefix(VPC_ENDPOINT_ID_PREFIX)?
        .split('-')
        .next()?;
    if !VPC_ENDPOINT_ID_HEX_LENGTHS.contains(&hex.len())
        || !hex
            .chars()
            .all(|glyph| glyph.is_ascii_hexdigit() && !glyph.is_ascii_uppercase())
    {
        return None;
    }
    // By `get`, never by a byte-index slice: the label is read off the wire, and the id's
    // own width is all that has been established about it.
    first.get(..VPC_ENDPOINT_ID_PREFIX.len() + hex.len())
}

/// The Region `host` addresses, read off its own labels, or `None` for a host that is not
/// an `execute-api` endpoint and so names no Region.
///
/// It is the Region of the ARN a statement of the api's resource policy names the method by
/// and a refusal reports, which is the Region the client addressed the endpoint in.
fn region_of_host(host: Option<&str>) -> Option<&str> {
    endpoint_labels(host?).map(|(_first, region)| region)
}

/// Whether `path` addresses the `@connections` space of ApiGatewayManagementApi, in either
/// of the two forms that service's callers use: with the stage prefix the SDK's endpoint
/// carries (`/{stage}/@connections/{id}`) and without it (`/@connections/{id}`).
fn addresses_connections(path: &str) -> bool {
    path_segments(path)
        .into_iter()
        .take(2)
        .any(|segment| segment == CONNECTIONS_SEGMENT)
}

/// Serve one request addressed to a deployed stage's invoke URL, from the control plane's
/// own records.
///
/// Free-standing and reached with the ONE shared engine, so the two seams that serve this
/// data plane — `Service::override_routing_error` for a request carrying no credential and
/// `CredentialedDataPlane` for one carrying a SigV4 credential — answer identically; they
/// differ only in whether the emulator verified a signature on the way in.
///
/// Every response leaves here NAMING THE REQUEST it answered, in the header API Gateway
/// reports a request id in: this is the one funnel both seams reach, so a served
/// integration, a gateway refusal and an endpoint that serves nothing all report one, and
/// a client can correlate the response it holds with the request the API processed.
/// `service_common` owns both the minting and the header, so this data plane reports the
/// id in the one spelling the rest of the emulator reports it in.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    ::service_common::query_error_envelope::with_request_id_header(serve_resolved(engine, req, 0))
}

/// The response this data plane's own records compute for `req`, before the request id is
/// reported on it — the resolution and the serving, which is everything the module docs
/// describe.
///
/// `hop` counts the passed-through integrations already followed to get here: a proxy
/// integration is served by passing the request on to this same function, so the count is
/// what bounds a chain of them ([`MAX_PASSED_THROUGH_HOPS`]).
fn serve_resolved<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    hop: u8,
) -> RtResponse {
    let addressed = Addressed::of_request(req);
    let Some(state) = crate::state::service_state(engine) else {
        return refusal_response(None, API_CONFIGURATION_ERROR);
    };
    // What this request RESOLVES to: the integration that is to answer it, what VALIDATING
    // its payload needs, and what METERING it is subject to. All three are read under a
    // shared read of the store, which is RELEASED before any of them runs: validating a
    // payload against a caller-stored JSON Schema is the one piece of work here whose cost
    // a caller's own records decide, so it must not hold the lock every other request to
    // this service waits on, and recording a metered request needs an exclusive lock of
    // its own.
    let (integration, pending_body, metering) = {
        let store = state.lock();
        let Some(resolved) = resolve(&store, &addressed) else {
            // The api was deleted between the claim and now, or another process minted the
            // host: an endpoint that serves no api of this account is `403 Forbidden`.
            return refusal_response(None, FORBIDDEN);
        };
        let api = resolved.api;
        if api.disable_execute_api_endpoint {
            // `disableExecuteApiEndpoint`: "Specifies whether clients can invoke your API by
            // using the default execute-api endpoint." Turned off, the endpoint serves nothing.
            return refusal_response(Some(api), FORBIDDEN);
        }

        let Some((stage_name, resource_path)) = split_stage(resolved.invoke_path) else {
            return refusal_response(Some(api), FORBIDDEN);
        };
        let Some(stage) = api.stages.get(stage_name) else {
            return refusal_response(Some(api), FORBIDDEN);
        };
        let Some(deployment) = api.deployments.get(&stage.deployment_id) else {
            return refusal_response(Some(api), FORBIDDEN);
        };

        // The DEPLOYMENT decides what is served: a method put on the API after the stage's
        // deployment was created is not in it, and one deleted afterwards still is.
        let Some(matched_path) = match_resource_path(deployment, &resource_path) else {
            return refusal_response(Some(api), MISSING_AUTHENTICATION_TOKEN);
        };
        let Some((verb, snapshot)) = match_method(deployment, matched_path, req.method()) else {
            return refusal_response(Some(api), MISSING_AUTHENTICATION_TOKEN);
        };

        let authorization_type = snapshot
            .authorization_type
            .as_deref()
            .unwrap_or(AUTHORIZATION_NONE);
        // The live method carries the integration, the authorizer and the request
        // validator the deployed method names; `Deployment$apiSummary`'s `MethodSnapshot`
        // models only `authorizationType` and `apiKeyRequired`, so the rest of the method
        // is read from the API itself.
        let live_method = api
            .resources
            .values()
            .find(|resource| resource.path == matched_path)
            .and_then(|resource| resource.methods.get(verb));

        if let Some(refusal) = authorization_refusal(api, live_method, authorization_type, req) {
            return refusal_response(Some(api), refusal);
        }
        // The api's own RESOURCE POLICY, which "applies to this RestApi regardless of the
        // caller and Method configuration" (`RestApi$policy`): a document the developer
        // attached decides every request this endpoint answers, so it is applied here —
        // before the api key, the request validator, the metering and the integration, none
        // of which a request it refuses ever reaches. An api carrying no document is asked
        // nothing and is served as it was before resource policies were read at all.
        if let Some(policy) = api
            .policy
            .as_deref()
            .filter(|policy| !policy.trim().is_empty())
        {
            // Who the request speaks for is the METHOD's authorization, not what the
            // request carries: API Gateway authenticates nobody for a method whose
            // `authorizationType` is `NONE`, so such a call speaks for nobody however it is
            // signed, while an `AWS_IAM` method is reached with a credential the emulator
            // has already verified. `service_common` owns the one definition of the identity
            // this emulator's published credential speaks for, so the principal a refusal
            // here blames is the one STS reports for it.
            let signed_as = (authorization_type == AUTHORIZATION_AWS_IAM)
                .then(::service_common::caller_principal::ambient_user_arn);
            let call = resource_policy::Call {
                region: region_of_host(addressed.host).unwrap_or(crate::state::DEFAULT_REGION),
                api_id: &api.id,
                stage: &stage.stage_name,
                verb,
                resource_path: matched_path,
                source_vpce: resolved.source_vpce,
                caller: match signed_as.as_deref() {
                    Some(principal) => resource_policy::Caller::Signed(principal),
                    None => resource_policy::Caller::Anonymous,
                },
            };
            if let Some(message) = resource_policy::denial(policy, &call) {
                tracing::warn!(
                    api_id = %api.id,
                    stage = %stage.stage_name,
                    path = %matched_path,
                    verb = %verb,
                    "the api's resource policy does not allow this invoke"
                );
                return refusal_response(Some(api), access_denied(message));
            }
        }
        // A key-required method is admitted by the key the records associate with this
        // stage, and the association is also what says which usage plans METER the call.
        let admission = if snapshot.api_key_required {
            match api_key_admission(&store, api, &stage.stage_name, req) {
                Some(admission) => Some(admission),
                None => return refusal_response(Some(api), INVALID_API_KEY),
            }
        } else {
            // A method that requires no key presents the records no key to meter: a usage
            // plan is what "specify[ies] who can access associated API stages", so it
            // meters the calls it admits.
            None
        };

        let call = MethodRequest::of_request(req);
        // The method's own request validator, before the integration is reached: a
        // request the method declares invalid never gets to the backend. The PARAMETERS
        // half is decided here, from the request's own query string and headers; the BODY
        // half is prepared here and decided below, off the lock.
        if let Some(refusal) = parameters_refusal(api, live_method, req) {
            return refusal_response(Some(api), refusal);
        }
        let pending_body = pending_body_check(api, live_method, &call);

        // The integration is what answers, and it is READ here and PERFORMED below, after
        // the payload check: the step list this module opens with has the request validator
        // ahead of the integration, so a payload the method's model does not admit is
        // refused with the integration never performed at all. Performing it is the half
        // this module shares with `TestInvokeMethod`, so the SAME records decide a
        // simulated invocation and a real one.
        let integration = ResolvedIntegration {
            api_id: api.id.clone(),
            stage: stage.stage_name.clone(),
            path: matched_path.to_string(),
            verb: verb.to_string(),
            // The private connection a passed-through integration is carried over is this
            // service's own record, so it is resolved here, under the one lock that read
            // the integration itself.
            unusable_connection: live_method
                .and_then(|method| method.integration.as_ref())
                .and_then(|integration| unusable_vpc_link(&store, integration)),
            method: live_method.cloned(),
            call,
        };
        // The 429 this call earns if the plans metering it have no request left for it,
        // rendered now because it is rendered from the api's own gateway responses.
        let metering = admission
            .filter(|admission| !admission.throttled_plans.is_empty())
            .map(|admission| (admission, refusal_response(Some(api), THROTTLED)));
        (integration, pending_body, metering)
    };

    // The usage plan METERS the call before its payload is checked, as API Gateway's
    // method-request flow does: a request the plan sheds is answered 429 whatever its
    // payload says. This is the exclusive-lock half, so it is taken on its own.
    if let Some((admission, throttled)) = metering {
        let mut store = state.lock();
        if metered_refusal(&mut store, &admission).is_some() {
            return throttled;
        }
    }

    // OFF the lock: the payload against the model's JSON Schema. A refused payload is
    // answered through the api's own gateway response for `BAD_REQUEST_BODY`, so the
    // records are read again — briefly, and only to render that refusal.
    if let Some(pending) = pending_body {
        if let Some(refusal) = pending.refusal(req.body().bytes()) {
            let store = state.lock();
            let api = resolve(&store, &addressed).map(|resolved| resolved.api);
            return refusal_response(api, refusal);
        }
    }

    // The request has got past everything the method declares about it, so now — and only
    // now — the integration is performed.
    //
    // Every outcome an integration this call cannot be performed through earns is rendered
    // the same way: through the api's own gateway response for the refusal, so the records
    // are read again — briefly, and only to render it.
    let refuse = |unserved: UnservedIntegration| -> RtResponse {
        tracing::warn!(
            api_id = %integration.api_id,
            stage = %integration.stage,
            path = %integration.path,
            verb = %integration.verb,
            reason = %unserved.reason,
            "the stage cannot serve this call through the method's integration"
        );
        let store = state.lock();
        let api = resolve(&store, &addressed).map(|resolved| resolved.api);
        refusal_response(api, unserved.refusal)
    };

    // An integration that PASSES THE REQUEST THROUGH to an endpoint this emulator serves is
    // performed by serving the request it passes on, so the answer is that endpoint's own.
    // It is reached before the api's own records are asked to produce a response, because
    // for such an integration they produce none.
    let passed_on = integration
        .method
        .as_ref()
        .and_then(|method| method.integration.as_ref())
        .and_then(|record| passed_through(record, req));
    if let Some(passed_on) = passed_on {
        if let Some(reason) = integration.unusable_connection.clone() {
            return refuse(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason,
            });
        }
        return match passed_on {
            PassedThrough::Unserved(unserved) => refuse(unserved),
            PassedThrough::Request(_) if hop >= MAX_PASSED_THROUGH_HOPS => {
                refuse(UnservedIntegration {
                    refusal: API_CONFIGURATION_ERROR,
                    reason: format!(
                        "the request has been passed through {MAX_PASSED_THROUGH_HOPS} \
                         integrations of this emulator's own endpoints and this one passes \
                         it on again, so it is not followed further"
                    ),
                })
            }
            PassedThrough::Request(onward) => serve_resolved(engine, &onward, hop + 1),
        };
    }

    match perform_integration(integration.method.as_ref(), &integration.call) {
        Ok(served) => served_response(served),
        Err(unserved) => refuse(unserved),
    }
}

/// The integration a resolved call is to be performed through, carried OUT of the store
/// lock so that it is performed only once the request has got past the method's request
/// validator and the usage plans metering it.
///
/// Owned rather than borrowed for the same reason [`PendingBodyCheck`] is: both run with
/// the lock released, and the integration is performed after the payload check, so what it
/// reads has to outlive the lock. The copy is of ONE method's own records — what a
/// control-plane call had to store for it — so it is bounded by the method's configuration
/// rather than by anything the invoke carries.
#[derive(Debug)]
struct ResolvedIntegration {
    /// The api the call resolved to, for the server log an integration this emulator
    /// cannot perform leaves.
    api_id: String,
    /// The stage it was invoked through, for that same log.
    stage: String,
    /// The resource path the request matched, for that same log.
    path: String,
    /// The verb the method is addressed by, for that same log.
    verb: String,
    /// The LIVE method the deployed one names, which carries the integration; `None` when
    /// the deployment holds a method the api no longer carries, which is an integration
    /// there is nothing to call.
    method: Option<MethodEntry>,
    /// Why the private connection the integration is carried over cannot carry this call,
    /// when it names one and it cannot ([`unusable_vpc_link`]).
    unusable_connection: Option<String>,
    /// What the integration reads off this method request.
    call: MethodRequest,
}

/// Record this request against every usage plan that meters the presented API key, and
/// report whether one of them sheds it.
///
/// A usage plan's `throttle` is "the request rate limits" the plan admits FOR EACH of its
/// keys, so the accounting is per `(plan, key)` pair
/// ([`crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState::throttle_admits`]):
/// a key metered by two plans has to satisfy both, and `Some(())` means at least one plan
/// had no request left for it at this instant, which is the `THROTTLED` gateway response.
///
/// Every plan is accounted even when an earlier one sheds the request: a shed request is
/// one the key SENT, and the plans that still had capacity are the ones that admitted it.
fn metered_refusal(
    store: &mut crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    admission: &Metered,
) -> Option<()> {
    let now = ::std::time::Instant::now();
    let mut shed = false;
    for (plan_id, limits) in &admission.throttled_plans {
        if !store.throttle_admits(plan_id, &admission.key_id, *limits, now) {
            shed = true;
        }
    }
    shed.then_some(())
}

/// The refusal the method's `authorizationType` earns this request, or `None` when the
/// request may proceed.
///
/// `NONE` admits every request. `AWS_IAM` needs a SigV4 credential, whose SIGNATURE the
/// emulator itself verifies (a service never checks signatures), so only its presence is
/// read here. An `Authorizer`-backed type is a decision this emulator cannot obtain — the
/// authorizer runs code — except for the one part of it that is settled before the
/// authorizer is called: an identity source the request does not carry is `401` without
/// the authorizer running at all (`Authorizer$identitySource`).
///
/// What `AWS_IAM` here does NOT do, stated so a passing test is not read as more than it
/// is: no IDENTITY-based decision is made. AWS both authenticates the signature and
/// evaluates `execute-api:Invoke` for the resolved principal against the caller's identity
/// policies as well as the api's resource policy, refusing an authenticated but
/// unprivileged caller. This emulator models no identity-policy evaluation for an invoke,
/// so the decision an under-privileged principal would get from AWS is not one it can
/// produce; a verified credential gets past THIS gate. The api's own RESOURCE policy is a
/// decision it does make, on every request whatever the authorization type —
/// [`super::resource_policy`], applied by [`serve_resolved`] straight after this.
fn authorization_refusal(
    api: &RestApiEntry,
    method: Option<&MethodEntry>,
    authorization_type: &str,
    req: &RtRequest,
) -> Option<GatewayRefusal> {
    if authorization_type == AUTHORIZATION_NONE {
        return None;
    }
    if authorization_type == AUTHORIZATION_AWS_IAM {
        return (!carries_sigv4_credential(req)).then_some(MISSING_AUTHENTICATION_TOKEN);
    }
    if !AUTHORIZATION_BY_AUTHORIZER.contains(&authorization_type) {
        // A value outside the documented set is a method this emulator cannot authorize.
        return Some(API_CONFIGURATION_ERROR);
    }
    // A method whose `authorizerId` names no authorizer of this API cannot authorize the
    // call at all, which is a configuration the stage cannot serve.
    let Some(authorizer) = method
        .and_then(|method| method.authorizer_id.as_deref())
        .and_then(|id| api.authorizers.get(id))
    else {
        return Some(API_CONFIGURATION_ERROR);
    };
    for source in authorizer
        .identity_source
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        // Each identity source the authorizer declares must be PRESENT, or the authorizer
        // is not called at all. The two request-bound forms are read here; a source bound
        // to something else (a stage variable, say) is not one this emulator can resolve,
        // and is left to the refusal below rather than treated as satisfied.
        if let Some(header) = source.strip_prefix(METHOD_REQUEST_HEADER_PREFIX) {
            if req
                .headers()
                .get(header.to_ascii_lowercase())
                .is_none_or(str::is_empty)
            {
                return Some(UNAUTHORIZED);
            }
            continue;
        }
        if let Some(parameter) = source.strip_prefix(METHOD_REQUEST_QUERY_PREFIX) {
            let present = query_of(req.uri()).split('&').any(|pair| {
                pair.split_once('=')
                    .is_some_and(|(key, value)| key == parameter && !value.is_empty())
            });
            if !present {
                return Some(UNAUTHORIZED);
            }
        }
    }
    tracing::warn!(
        authorizer = %authorizer.name,
        kind = %authorizer.r#type,
        "the authorizer's decision cannot be obtained: this emulator runs no authorizer code"
    );
    Some(AUTHORIZER_CONFIGURATION_ERROR)
}

/// What the records say about the API key a request presented: which stored key admitted
/// the call, and the usage plans that METER it for this stage.
///
/// The key id rather than the key's value, because the value is the client's credential
/// and the accounting is per stored key; the plans are the ones whose throttle declares a
/// limit at all, so a plan that limits nothing adds no accounting.
#[derive(Debug)]
struct Metered {
    /// The stored `ApiKey`'s id.
    key_id: String,
    /// The `(usage plan id, limits)` pairs this call is metered against.
    throttled_plans: Vec<(String, crate::state::ThrottleLimits)>,
}

/// The admission the API key a request presents earns at `stage`, or `None` when the
/// records associate no such key with it.
///
/// The key travels in [`API_KEY_HEADER`] when the API's `apiKeySource` is `HEADER`; the
/// other documented source reads it from a custom authorizer's returned context, which is
/// a decision this emulator cannot obtain, so no key is presented for it.
///
/// A key admits the call when it is ENABLED and this api's `stage` is among the stages the
/// records associate it with — see [`plans_metering`] and `ApiKey$stageKeys` for the two
/// associations that say so. A value no stored key carries, a disabled key and a key
/// associated with some other stage are each a key that does not admit the call.
fn api_key_admission(
    store: &crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    api: &RestApiEntry,
    stage: &str,
    req: &RtRequest,
) -> Option<Metered> {
    let source = api
        .api_key_source
        .as_deref()
        .unwrap_or(crate::state::DEFAULT_API_KEY_SOURCE);
    if source != API_KEY_SOURCE_HEADER {
        return None;
    }
    let presented = req.headers().get(API_KEY_HEADER).filter(|key| !key.is_empty())?;
    store
        .api_keys
        .values()
        .filter(|key| key.value == presented && key.enabled)
        .find_map(|key| {
            let associating = plans_associating(store, key, api, stage);
            // The key's own `stageKeys` is the OLDER association, which
            // `CreateApiKey$stageKeys` documents as "DEPRECATED FOR USAGE PLANS -
            // Specifies stages associated with the API key". It is honoured beside the
            // plan because the operations that write it still accept it, and a deprecated
            // association is still an association — but it names no plan, so it carries no
            // limits to meter against.
            let by_stage_key = key.stage_keys.contains(&format!("{}/{stage}", api.id));
            (!associating.is_empty() || by_stage_key).then(|| Metered {
                key_id: key.id.clone(),
                // Only a plan whose throttle declares a limit meters anything: a plan
                // without one admits every request it associates.
                throttled_plans: associating
                    .into_iter()
                    .filter_map(|(plan_id, limits)| limits.map(|limits| (plan_id, limits)))
                    .collect(),
            })
        })
}

/// The usage plans that associate `key` with `stage` of `api`, each with the limits its
/// `throttle` declares — `None` for a plan that declares none.
///
/// A `UsagePlan` is "used to specify who can access associated API stages", its
/// `apiStages` name the api and stage its limits apply to, and a `UsagePlanKey` is the
/// association of one stored API key with one plan. So a client creates the key, a plan
/// over the api stage, and a plan key — and that key is what the stage admits, metered by
/// that plan.
fn plans_associating(
    store: &crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    key: &ApiKeyEntry,
    api: &RestApiEntry,
    stage: &str,
) -> Vec<(String, Option<crate::state::ThrottleLimits>)> {
    store
        .usage_plans
        .values()
        .filter(|plan| {
            plan.keys.contains_key(&key.id)
                && plan.api_stages.iter().any(|api_stage| {
                    api_stage.api_id.as_deref() == Some(api.id.as_str())
                        && api_stage.stage.as_deref() == Some(stage)
                })
        })
        .map(|plan| {
            (
                plan.id.clone(),
                crate::state::ThrottleLimits::declared(plan.throttle.as_ref()),
            )
        })
        .collect()
}

/// Where a request parameter a method declares travels on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParameterIn {
    /// The query string, in either its single- or multi-value spelling.
    Query,
    /// A request header, in either spelling.
    Header,
    /// A path segment of the resource the method sits on.
    Path,
}

/// Where `expression` says a request parameter travels and under what NAME, or `None` for
/// a key that is no `method.request.{location}.{name}` mapping expression at all.
fn parameter_source(expression: &str) -> Option<(ParameterIn, &str)> {
    // The multi-value spellings are tried first: each is a longer prefix than the
    // single-value one it extends, and both name the same parameter.
    [
        (METHOD_REQUEST_MULTI_QUERY_PREFIX, ParameterIn::Query),
        (METHOD_REQUEST_QUERY_PREFIX, ParameterIn::Query),
        (METHOD_REQUEST_MULTI_HEADER_PREFIX, ParameterIn::Header),
        (METHOD_REQUEST_HEADER_PREFIX, ParameterIn::Header),
        (METHOD_REQUEST_PATH_PREFIX, ParameterIn::Path),
    ]
    .into_iter()
    .find_map(|(prefix, source)| {
        expression
            .strip_prefix(prefix)
            .filter(|name| !name.is_empty())
            .map(|name| (source, name))
    })
}

/// The `RequestValidator` the method names, or `None` when it names none the API holds.
///
/// A `Method` names its validator in `requestValidatorId`, and the `RequestValidator` that
/// id resolves to declares which halves of the request are checked
/// (`validateRequestParameters`, `validateRequestBody`, each `@default false`): a method
/// that names none, or names one of the two flags only, is checked exactly as far as it
/// declares — which is why the four modes differ in what they refuse. A
/// `requestValidatorId` naming no validator of the API validates nothing: that is a
/// configuration this emulator cannot apply, and inventing a refusal for it would refuse
/// requests AWS serves.
fn declared_validator<'a>(
    api: &'a RestApiEntry,
    method: Option<&MethodEntry>,
) -> Option<&'a crate::state::RequestValidatorEntry> {
    api.request_validators
        .get(method?.request_validator_id.as_deref()?)
}

/// The refusal the PARAMETERS half of the method's request validator earns this request,
/// or `None` when it declares no parameter check or the request carries everything the
/// method declares required.
///
/// The parameters are checked FIRST, before the payload: a request that does not even
/// carry what the method declares required is refused for that rather than for whatever
/// its payload says.
fn parameters_refusal(
    api: &RestApiEntry,
    method: Option<&MethodEntry>,
    req: &RtRequest,
) -> Option<GatewayRefusal> {
    let validator = declared_validator(api, method)?;
    if !validator.validate_request_parameters {
        return None;
    }
    let missing = missing_parameters(method?, req);
    (!missing.is_empty()).then(|| bad_request_parameters(&missing))
}

/// The names of the request parameters the method declares REQUIRED that this request does
/// not carry, in the order the method declares them.
///
/// `Method$requestParameters` is "a key-value map defining required or optional method
/// request parameters … the value is a Boolean flag indicating whether the parameter is
/// required", so only the `true` ones can be missing. A parameter is carried when it is
/// present and NOT BLANK, which is what request validation verifies of a query-string
/// parameter or a header.
///
/// A PATH parameter is never missing: the request path matched the resource template that
/// declares it, which cannot match with that segment absent.
fn missing_parameters(method: &MethodEntry, req: &RtRequest) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for (expression, required) in &method.request_parameters {
        if !required {
            continue;
        }
        let Some((source, name)) = parameter_source(expression) else {
            continue;
        };
        let carried = match source {
            ParameterIn::Query => query_of(req.uri()).split('&').any(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                key == name && !value.is_empty()
            }),
            ParameterIn::Header => req
                .headers()
                .get(name.to_ascii_lowercase())
                .is_some_and(|value| !value.is_empty()),
            ParameterIn::Path => true,
        };
        if !carried && !missing.iter().any(|already| already == name) {
            missing.push(name.to_string());
        }
    }
    missing
}

/// The URL the model `model_name` of the API `api_id` is a JSON Schema resource under —
/// the identifier API Gateway gives a model's schema, which is what a `$ref` from another
/// model of the same API names.
fn model_schema_url(api_id: &str, model_name: &str) -> String {
    format!("https://apigateway.amazonaws.com/restapis/{api_id}/models/{model_name}")
}

/// The schema resources this validator resolves a `$ref` against: NOTHING but what this
/// module registered itself.
///
/// A `Model`'s schema is a document any caller stores, and `$ref` is a dynamic resolution
/// mechanism, so a schema could name a resource by any URL — including a `file:` one.
/// Registering this as the compiler's loader REPLACES the one it comes with, so a
/// reference the API's own models do not satisfy resolves to an error instead of reaching
/// the host's filesystem or network: the only resources are the ones
/// [`PendingBodyCheck::refusal`] adds, which are models of the api being invoked.
#[derive(Debug)]
struct NoSchemasBeyondTheApi;

impl ::boon::UrlLoader for NoSchemasBeyondTheApi {
    fn load(&self, url: &str) -> Result<::serde_json::Value, Box<dyn ::std::error::Error>> {
        tracing::warn!(
            reference = %url,
            "the model's schema references a resource outside the api's own models, which \
             this emulator resolves nothing for"
        );
        Err("this emulator resolves no schema outside the api's own models".into())
    }
}

/// What validating one request's PAYLOAD needs, owned: the schema to validate it against
/// and the sibling schemas that schema references.
///
/// Owned rather than borrowed on purpose. Compiling and evaluating a JSON Schema is the
/// one piece of work in this data plane whose cost a caller's own RECORDS decide, so it is
/// done with the shared store's lock RELEASED (see [`serve_resolved`]) — one request's
/// payload check cannot hold up every other request to this service. The copy is of the
/// stored schema text, which `CreateModel` bounds at
/// [`crate::ops::create_model::MAX_MODEL_SCHEMA_CHARS`], and of the siblings that schema
/// names, so the work and the copy are bounded by records a control-plane call had to
/// store rather than by anything the invoke carries.
#[derive(Debug)]
struct PendingBodyCheck {
    /// The api whose model this is, for the log and the schema's resource URL.
    api_id: String,
    /// The model the method declared for this request's content type.
    model_name: String,
    /// That model's schema, as stored.
    schema: String,
    /// The sibling models the schema REFERENCES by their model-schema URL, as
    /// `(url, stored schema)`.
    referenced: Vec<(String, String)>,
}

/// The payload check the method's request validator declares for this call, or `None` when
/// it declares none that can be applied.
///
/// `Method$requestModels` is "a key-value map specifying data schemas … a key is a content
/// type and the value is a `Model` name", so a content type the method maps no model to
/// declares nothing to validate against. `None` is also the answer when the model names no
/// record of the API or carries no schema: each is a configuration this emulator cannot
/// validate against, and inventing a refusal for it would refuse requests AWS serves.
fn pending_body_check(
    api: &RestApiEntry,
    method: Option<&MethodEntry>,
    call: &MethodRequest,
) -> Option<PendingBodyCheck> {
    let validator = declared_validator(api, method)?;
    if !validator.validate_request_body {
        return None;
    }
    let model_name = method?.request_models.get(call.media_type())?;
    let schema = api.models.get(model_name)?.schema.as_deref()?.to_string();
    // A schema composed of references to OTHER models of the API resolves them: API
    // Gateway identifies a model's schema by [`model_schema_url`], so a sibling the schema
    // REFERENCES BY THAT URL is carried over with it. Only a referenced sibling is, so the
    // work one request causes is bounded by the references the schema carries rather than
    // by how many models the API holds.
    let referenced = if schema.contains("$ref") {
        api.models
            .iter()
            .filter(|(name, _)| *name != model_name)
            .filter_map(|(name, sibling)| {
                let url = model_schema_url(&api.id, name);
                let text = sibling.schema.clone()?;
                schema.contains(&url).then_some((url, text))
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(PendingBodyCheck {
        api_id: api.id.clone(),
        model_name: model_name.clone(),
        schema,
        referenced,
    })
}

impl PendingBodyCheck {
    /// The refusal this request's payload earns against the model, or `None` when it
    /// satisfies it.
    ///
    /// `CreateModel$schema` is "the schema for the model. For application/json models,
    /// this should be JSON schema draft 4 model", so the payload is validated against that
    /// document by a JSON Schema validator with draft 4 as the default dialect for a schema
    /// that names none. A payload that is not a JSON document at all does not satisfy a
    /// JSON Schema, so an unparseable (including an empty) body is refused exactly as a
    /// structurally wrong one is; a body the transport did not buffer in memory
    /// (`None`) is one this emulator cannot read, so it is validated against nothing.
    ///
    /// A stored schema this validator cannot READ as a JSON Schema — a document that is a
    /// JSON object, which is all `CreateModel` requires of it, but no schema, or one whose
    /// references resolve to nothing — is a method the stage cannot serve as configured:
    /// the validator the method declares cannot be applied to anything, so the call is
    /// answered through `API_CONFIGURATION_ERROR` exactly as an integration this module
    /// cannot perform is, rather than served as if the check had passed.
    ///
    /// The work is bounded by its two inputs: the schema is at most
    /// [`crate::ops::create_model::MAX_MODEL_SCHEMA_CHARS`] and the payload is what the
    /// front door admitted, and a JSON Schema evaluation applies each keyword of the one
    /// to the members of the other. Nothing here compiles a pattern into a backtracking
    /// engine either — the validator's regexes are the `regex` crate's, which has no
    /// backtracking — so a stored `pattern` cannot make the evaluation super-linear in the
    /// payload.
    fn refusal(&self, payload: Option<&[u8]>) -> Option<GatewayRefusal> {
        let Ok(schema) = ::serde_json::from_str::<::serde_json::Value>(&self.schema) else {
            return Some(self.unapplicable_schema("it is not a JSON document"));
        };
        let Ok(instance) = ::serde_json::from_slice::<::serde_json::Value>(payload?) else {
            return Some(bad_request_body(unreadable_payload_detail()));
        };

        let mut compiler = ::boon::Compiler::new();
        // The only schema resources are the ones registered below: see
        // [`NoSchemasBeyondTheApi`] for why the compiler's own loader is replaced.
        compiler.use_loader(Box::new(NoSchemasBeyondTheApi));
        // "For application/json models, this should be JSON schema draft 4 model"
        // (`CreateModel$schema`), so draft 4 is the dialect a schema that declares no
        // `$schema` of its own is read under; one that declares a dialect keeps it.
        compiler.set_default_draft(::boon::Draft::V4);
        for (url, text) in &self.referenced {
            if let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(text) {
                let _ = compiler.add_resource(url, document);
            }
        }
        let url = model_schema_url(&self.api_id, &self.model_name);
        if compiler.add_resource(&url, schema).is_err() {
            return Some(self.unapplicable_schema("it is not a schema resource"));
        }
        let mut schemas = ::boon::Schemas::new();
        let Ok(compiled) = compiler.compile(&url, &mut schemas) else {
            return Some(self.unapplicable_schema(
                "it is not a JSON Schema this emulator can compile, or it references a \
                 resource that resolves to nothing",
            ));
        };
        schemas
            .validate(&instance, compiled)
            .err()
            .map(|failed| bad_request_body(validation_detail(&failed)))
    }

    /// The refusal a stored schema the validator cannot be built from earns, said once:
    /// the method declares a check its own records make impossible, which is a
    /// configuration the stage cannot serve the call through.
    fn unapplicable_schema(&self, reason: &str) -> GatewayRefusal {
        tracing::warn!(
            api_id = %self.api_id,
            model = %self.model_name,
            reason = %reason,
            "the method's request validator cannot be applied: the model's schema is not \
             one this emulator can validate against"
        );
        API_CONFIGURATION_ERROR
    }
}

/// The validation detail a payload that is not a JSON document at all earns: there is no
/// schema finding to report, because the document the schema would be applied to could not
/// be read.
fn unreadable_payload_detail() -> String {
    "[the request body could not be read as a JSON document]".to_string()
}

/// The validation's own finding about the payload, as one line, for
/// `$context.error.validationErrorString`.
///
/// It is the VALIDATOR's report — which member of the payload failed which keyword of the
/// model's schema — flattened to one line and bracketed the way API Gateway brackets that
/// member in a gateway response, so a developer reading the refusal can see which part of
/// the payload the model rejected. Nothing of it is written here: the schema's own URL line
/// is dropped (it names the model this emulator compiled, not the payload) and the rest is
/// the validator's text.
fn validation_detail(failed: &::boon::ValidationError) -> String {
    let reported = failed.to_string();
    let findings = reported
        .split_once('\n')
        .map(|(_schema, findings)| findings)
        .unwrap_or(reported.as_str());
    let flattened: Vec<&str> = findings
        .split_whitespace()
        .filter(|word| *word != "-")
        .collect();
    format!("[{}]", flattened.join(" "))
}

/// What an integration reads off the METHOD REQUEST it serves: the media type the request
/// declares, which selects the integration REQUEST template, and the media type the caller
/// accepts, which selects the integration RESPONSE template.
///
/// Both callers of [`perform_integration`] build one — the data plane off the wire
/// ([`Self::of_request`]), `TestInvokeMethod` out of the members a caller simulates a
/// request with ([`Self::simulated`]) — so an invocation that reached a socket and one that
/// never did select the same templates from the same records.
#[derive(Debug)]
pub struct MethodRequest {
    /// The media type the request declares, as a `requestTemplates` key is compared.
    media_type: String,
    /// The first media type of the request's `Accept`, or `None` when it accepts anything.
    accepted: Option<String>,
}

impl MethodRequest {
    /// What a request off the wire declares and accepts.
    fn of_request(req: &RtRequest) -> Self {
        Self {
            media_type: declared_media_type(req.headers().get("content-type")),
            accepted: first_media_type(req.headers().get("accept")),
        }
    }

    /// What a SIMULATED request declares and accepts, from the two header values a caller
    /// supplied as members rather than on the wire.
    pub fn simulated(content_type: Option<&str>, accept: Option<&str>) -> Self {
        Self {
            media_type: declared_media_type(content_type),
            accepted: first_media_type(accept),
        }
    }

    /// The media type this request declares; named in the sentence a refusal of an
    /// unmapped content type carries.
    pub fn media_type(&self) -> &str {
        &self.media_type
    }
}

/// The response an integration this emulator CAN perform produces, every part of it
/// computed from the API's own records.
#[derive(Debug)]
pub struct IntegratedResponse {
    /// The status code of the integration response the call mapped to.
    pub status: u16,
    /// The media type the rendered body is keyed by.
    pub content_type: String,
    /// The rendered body, empty when no response template applies.
    pub body: Vec<u8>,
    /// The mapped response headers the method response declares, keyed by the header name
    /// as the mapping declares it. `content-type` is NOT among them: it is
    /// [`Self::content_type`].
    pub headers: BTreeMap<String, String>,
}

/// An integration this call cannot be performed through: the GATEWAY RESPONSE it is
/// answered by, and why in one sentence — for the server log, and for the execution log
/// `TestInvokeMethod` reports to its caller.
#[derive(Debug)]
pub struct UnservedIntegration {
    /// The refusal the call earns; see [`GatewayRefusal`].
    pub refusal: GatewayRefusal,
    /// Why the integration could not be performed. States the configuration, never a
    /// backend answer, because there is no backend answer.
    pub reason: String,
}

/// What a PASSED-THROUGH integration makes of this request.
#[derive(Debug)]
enum PassedThrough {
    /// The request to serve against the endpoint the integration's `uri` names: the
    /// caller's own request, re-addressed at that endpoint and carrying the headers the
    /// integration maps onto it.
    ///
    /// Boxed: a whole request is far larger than the refusal beside it, and this value
    /// is returned by value from the resolution below.
    Request(Box<RtRequest>),
    /// The integration passes the request to a backend, or through a mapping, this
    /// emulator cannot perform.
    Unserved(UnservedIntegration),
}

/// How `integration` passes this request on, or `None` when it passes nothing on — so the
/// call is performed from the api's own records instead ([`perform_integration`]).
///
/// Only an `HTTP_PROXY` whose `uri` names an `execute-api` INVOKE ENDPOINT passes anything
/// on here, and the destination is checked against that shape BEFORE any request is built:
/// such an endpoint is one this emulator serves itself, so the pass-through is a request
/// handed back to [`serve_resolved`] in process — no connection is opened, no name is
/// resolved and no address the caller chose is dialled. The endpoint's own records then
/// decide the answer — which api, which stage, which method, and that api's resource policy
/// — exactly as they would for a client that addressed the endpoint directly, so a proxy
/// integration's answer is never assembled here.
///
/// A `uri` naming any other host is a backend over the network, and an `HTTP` integration's
/// response is chosen by a mapping this emulator does not evaluate: both are left to
/// [`perform_integration`]'s refusal. A `uri` carrying a mapping placeholder, and a
/// `requestParameters` entry outside the two forms that member documents (a static value in
/// single quotes, or a `method.request.{header|querystring}.{name}` source) onto a header of
/// the integration request, are stated as integrations this call cannot perform rather than
/// silently dropped. A mapping whose SOURCE the request simply does not carry is a different
/// thing from one written in a form that is not read: the header is left off, and the
/// request is still passed on ([`MappedParameter`]).
fn passed_through(integration: &GetIntegrationOutput, req: &RtRequest) -> Option<PassedThrough> {
    if integration.r#type.as_deref() != Some(HTTP_PROXY_INTEGRATION_TYPE) {
        return None;
    }
    let uri = integration.uri.as_deref().unwrap_or_default();
    // The host the request is passed on to. A `uri` with no authority at all addresses
    // nothing, and one whose authority is not an `execute-api` endpoint addresses a backend
    // outside this emulator: neither is served here.
    let host = authority_of(uri)?;
    endpoint_labels(host)?;
    if uri.contains('{') {
        return Some(PassedThrough::Unserved(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration's uri carries a mapping placeholder, which this \
                     emulator does not expand: no request is passed on to a guessed-at \
                     address"
                .to_string(),
        }));
    }
    let mut passed = RtRequest::new(SdkBody::from(request_payload(req)));
    if passed.set_uri(onward_uri(uri, req.uri()).as_str()).is_err() {
        return Some(PassedThrough::Unserved(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration's uri is not a URI a request can be addressed at"
                .to_string(),
        }));
    }
    // "Specifies the integration's HTTP method type" (`Integration$httpMethod`): the verb
    // the backend is called with, which is the caller's own where the integration names
    // none.
    let verb = integration
        .http_method
        .as_deref()
        .unwrap_or_else(|| req.method());
    if passed.set_method(verb).is_err() {
        return Some(PassedThrough::Unserved(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration's httpMethod is not an HTTP method a request can be \
                     made with"
                .to_string(),
        }));
    }
    for (name, value) in req.headers().iter() {
        if HEADERS_NOT_PASSED_THROUGH.contains(&name.to_ascii_lowercase().as_str()) {
            continue;
        }
        // `append`, not `insert`: `iter` yields one entry per value, so a repeated header
        // reaches the backend with every value it arrived with. Through the FALLIBLE form:
        // these are bytes the caller chose, and one the HTTP layer cannot represent must
        // leave the header off rather than abort the request.
        if let Err(refused) = passed
            .headers_mut()
            .try_append(name.to_string(), value.to_string())
        {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the caller's header cannot be carried onward and is left off the request \
                 the integration passes on"
            );
        }
    }
    for (key, value) in integration.request_parameters.iter().flatten() {
        let Some(name) = key.strip_prefix(INTEGRATION_REQUEST_HEADER_PREFIX) else {
            return Some(PassedThrough::Unserved(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason: format!(
                    "the integration maps {key}, which is not a header of the integration \
                     request: this emulator passes a request through rather than rewriting \
                     its address, so the mapping is not applied and the request is not sent"
                ),
            }));
        };
        let mapped = match mapped_request_parameter(value, req) {
            MappedParameter::Value(mapped) => mapped,
            // The source names a location this emulator reads and the request carries
            // nothing there, so the mapping maps nothing: the backend is called without
            // that header, which is what it would be called with on AWS.
            MappedParameter::SourceAbsent => continue,
            MappedParameter::NotRead => {
                return Some(PassedThrough::Unserved(UnservedIntegration {
                    refusal: API_CONFIGURATION_ERROR,
                    reason: format!(
                        "the integration maps {key} from {value}, which is neither a static \
                         value in single quotes nor a method request parameter this emulator \
                         reads"
                    ),
                }));
            }
        };
        // A mapped header REPLACES what the caller sent under that name, which is what a
        // mapping onto it declares; a value the wire cannot carry is stated rather than
        // dropped, because the backend would then be called without it.
        if passed
            .headers_mut()
            .try_insert(name.to_string(), mapped)
            .is_err()
        {
            return Some(PassedThrough::Unserved(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason: format!("the value the integration maps onto {key} is not one a \
                                 header can carry"),
            }));
        }
    }
    Some(PassedThrough::Request(Box::new(passed)))
}

/// The URI the onward request is addressed at: the integration's `uri`, carrying the
/// CALLER's query string as well as any the `uri` itself names.
///
/// A proxy integration passes the request through, and a request's query string is part of
/// it: a backend method that declares a query parameter must see the one the caller sent.
/// The `uri`'s own pairs come first — they are the configuration, and a caller cannot
/// displace them — and the caller's follow; a caller that sent none leaves the `uri`
/// untouched.
///
/// What does NOT travel is the credential a PRESIGNED caller carries in the query string
/// ([`SIGNED_QUERY_PARAMETERS`]), for the reason [`HEADERS_NOT_PASSED_THROUGH`] gives for
/// the header form of the same credential: a signature over the address the caller used
/// authenticates nothing at the address the request is passed on to.
fn onward_uri(uri: &str, request_uri: &str) -> String {
    let sent: Vec<&str> = query_of(request_uri)
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let name = pair.split('=').next().unwrap_or(pair).to_ascii_lowercase();
            !SIGNED_QUERY_PARAMETERS.contains(&name.as_str())
        })
        .collect();
    if sent.is_empty() {
        return uri.to_string();
    }
    let sent = sent.join("&");
    let separator = if query_of(uri).is_empty() { '?' } else { '&' };
    format!("{uri}{separator}{sent}")
}

/// What one `requestParameters` entry maps onto the integration request.
#[derive(Debug)]
enum MappedParameter {
    /// The value to carry onward.
    Value(String),
    /// The entry names a method request parameter this emulator reads and the request
    /// carries none under that name, so the mapping maps nothing at all.
    SourceAbsent,
    /// The entry is written in a form this emulator does not read, so what it would carry
    /// is unknown.
    NotRead,
}

/// What the `requestParameters` value `value` maps onto the integration request.
///
/// The member documents the two forms: "a method request parameter value or static value
/// that must be enclosed within single quotes". The static one is the literal between the
/// quotes; a method request parameter is read off the request this call arrived as — its
/// headers and its query string, the two locations a passed-through request carries whole.
/// The query string is read through `service_common`'s one definition of what a query
/// parameter's value is, so this reads a pair the way the rest of the emulator does; it is
/// the FIRST value under that name, which is the one a single-valued mapping expression
/// names ([`METHOD_REQUEST_MULTI_QUERY_PREFIX`] is the spelling that names them all).
fn mapped_request_parameter(value: &str, req: &RtRequest) -> MappedParameter {
    if is_static_mapping_value(value) {
        return match value
            .strip_prefix('\'')
            .and_then(|rest| rest.strip_suffix('\''))
        {
            Some(literal) => MappedParameter::Value(literal.to_string()),
            None => MappedParameter::NotRead,
        };
    }
    if let Some(name) = value.strip_prefix(METHOD_REQUEST_HEADER_PREFIX) {
        return match req.headers().get(name) {
            Some(sent) => MappedParameter::Value(sent.to_string()),
            None => MappedParameter::SourceAbsent,
        };
    }
    if let Some(name) = value.strip_prefix(METHOD_REQUEST_QUERY_PREFIX) {
        return match ::service_common::query_parameter(query_of(req.uri()), name) {
            Some(sent) => MappedParameter::Value(sent.to_string()),
            None => MappedParameter::SourceAbsent,
        };
    }
    MappedParameter::NotRead
}

/// The payload of a request as the bytes it is passed on with.
///
/// A body this emulator cannot read as bytes — one that was never buffered — passes nothing
/// on, which is what a request with no payload passes on too: every request this data plane
/// is handed arrives buffered.
fn request_payload(req: &RtRequest) -> Vec<u8> {
    req.body().bytes().unwrap_or_default().to_vec()
}

/// Why the private connection `integration` reaches its endpoint through cannot carry this
/// call, or `None` when it names no private connection or names one that can.
///
/// `connectionType` `VPC_LINK` is "for private connections between API Gateway and a network
/// load balancer in a VPC" and `connectionId` names the link that connection runs over, so
/// the link is this service's OWN record and is read while the store is held. A link this
/// account does not hold, and one that has not finished provisioning
/// ([`crate::state::VPC_LINK_AVAILABLE`]), carries no traffic — so the integration cannot be
/// performed over it, whatever its `uri` names.
fn unusable_vpc_link(
    store: &crate::state::ComAmazonawsApigatewayBackplanecontrolserviceState,
    integration: &GetIntegrationOutput,
) -> Option<String> {
    let connection_type = integration
        .connection_type
        .as_deref()
        .unwrap_or(DEFAULT_CONNECTION_TYPE);
    if connection_type != VPC_LINK_CONNECTION_TYPE {
        return None;
    }
    let Some(connection_id) = integration.connection_id.as_deref() else {
        return Some(format!(
            "the integration's connectionType is {VPC_LINK_CONNECTION_TYPE} and it names no \
             connectionId, so it reaches no network load balancer"
        ));
    };
    // "Specify this value only if you specify VPC_LINK as the connection type": the value
    // may also be a stage variable, which names the link indirectly.
    if connection_id.contains("${") {
        return Some(
            "the integration's connectionId is a stage variable, which this emulator does \
             not resolve to a VPC link"
                .to_string(),
        );
    }
    match store.vpc_links.get(connection_id) {
        None => Some(format!(
            "the integration connects through the VPC link {connection_id}, which this \
             account holds no record of"
        )),
        Some(link) if link.status() != crate::state::VPC_LINK_AVAILABLE => Some(format!(
            "the VPC link {connection_id} this integration connects through is {status}, so \
             it carries no traffic",
            status = link.status()
        )),
        Some(_) => None,
    }
}

/// Perform the call through the method's integration, answering the response the API's
/// records produce for it.
///
/// `MOCK` is the one type that needs no backend, and it is performed in full from those
/// records: the integration REQUEST template this call selects IS what the integration
/// returns, and the `statusCode` that template carries names the integration response the
/// call maps to. A call no template maps is passed through or refused as
/// `passthroughBehavior` says, and one whose mapping names no stored response maps to the
/// DEFAULT integration response — the one with no `selectionPattern`, since a `MOCK`
/// produces no backend response for a pattern to match.
///
/// Every other type names a backend whose answer is not this api's records to produce: an
/// `HTTP_PROXY` naming an endpoint of this emulator is served by passing the request on
/// ([`passed_through`], which [`serve_resolved`] reaches before this), and every other one
/// is an integration this call cannot perform rather than one whose answer is invented —
/// see the module docs.
pub fn perform_integration(
    method: Option<&MethodEntry>,
    call: &MethodRequest,
) -> Result<IntegratedResponse, UnservedIntegration> {
    let Some(integration) = method.and_then(|method| method.integration.as_ref()) else {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the method carries no integration, so there is nothing to call"
                .to_string(),
        });
    };
    let integration_type = integration.r#type.as_deref().unwrap_or_default();
    if integration_type != MOCK_INTEGRATION_TYPE {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the {integration_type} integration names a backend this emulator cannot \
                 call: no deployment package runs in process and no outbound connection is \
                 made, so no backend answer exists to map"
            ),
        });
    }

    let mapped_status = match request_mapping(integration, call) {
        RequestMapping::Mapped(template) => mock_status_code(template),
        RequestMapping::PassedThrough => None,
        RequestMapping::Unsupported => {
            return Err(UnservedIntegration {
                refusal: UNSUPPORTED_MEDIA_TYPE,
                reason: format!(
                    "no integration request template maps {}, and the integration's \
                     passthroughBehavior does not pass an unmapped body through",
                    call.media_type
                ),
            });
        }
    };
    let selected = mapped_status
        .and_then(|code| {
            integration
                .integration_responses
                .as_ref()
                .and_then(|responses| responses.get(&code))
        })
        .or_else(|| default_integration_response(integration));
    let Some(selected) = selected else {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the MOCK integration has no integration response for this call, so it \
                     maps to no method response"
                .to_string(),
        });
    };
    let Some(status) = selected
        .status_code
        .as_deref()
        .and_then(|code| code.parse::<u16>().ok())
    else {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration response this call maps to carries no status code the \
                     response can be sent with"
                .to_string(),
        });
    };

    let (content_type, body) = response_body(selected, call).map_err(|template| {
        UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the {} response template of {} bytes is a Velocity template, which this \
                 emulator does not evaluate: no body is invented for it",
                template.content_type, template.bytes
            ),
        }
    })?;
    Ok(IntegratedResponse {
        status,
        content_type,
        body,
        headers: mapped_response_headers(method, selected, status),
    })
}

/// The response headers the call answers with: API Gateway returns only the headers the
/// METHOD response declares, mapped as the INTEGRATION response says.
///
/// A `MOCK` has no backend response to map from, so the one mapping it can resolve is the
/// documented static value in single quotes ([`is_static_mapping_value`]): the literal
/// between the quotes is the header's value. Read by stripping the quotes rather than by an
/// index, so neither the read nor the length arithmetic can fail whatever was stored.
fn mapped_response_headers(
    method: Option<&MethodEntry>,
    selected: &GetIntegrationResponseOutput,
    status: u16,
) -> BTreeMap<String, String> {
    let declared = method
        .and_then(|method| method.method_responses.get(&status.to_string()))
        .and_then(|response| response.response_parameters.as_ref());
    let mut mapped = BTreeMap::new();
    for (key, value) in selected.response_parameters.iter().flatten() {
        let Some(name) = key.strip_prefix(METHOD_RESPONSE_HEADER_PREFIX) else {
            continue;
        };
        if declared.is_none_or(|declared| !declared.contains_key(key)) {
            continue;
        }
        let literal = is_static_mapping_value(value)
            .then(|| value.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')))
            .flatten();
        if let Some(literal) = literal {
            mapped.insert(name.to_string(), literal.to_string());
        }
    }
    mapped
}

/// The wire response a performed integration answers with.
///
/// A mapped header's name is lowercased on the way out, as HTTP/2 carries one; the name the
/// mapping declared is kept on [`IntegratedResponse::headers`] for the caller that REPORTS
/// the response instead of sending it.
///
/// A header is inserted with `try_insert`, not the panicking `insert`: its name and
/// value come from records a control-plane request wrote, and a stored mapping the wire
/// cannot carry (a value with a newline in it, say) must not fail the REQUEST — it is
/// skipped, and said so, so the rest of the response is still served.
fn served_response(served: IntegratedResponse) -> RtResponse {
    let mut response = response_with(served.status, &served.content_type, served.body);
    for (name, value) in served.headers {
        if let Err(refused) = response
            .headers_mut()
            .try_insert(name.to_ascii_lowercase(), value)
        {
            tracing::warn!(
                header = %name,
                error = %refused,
                "the mapped response header cannot be sent on the wire and is skipped"
            );
        }
    }
    response
}

/// What the integration's request templates and `passthroughBehavior` make of this call.
#[derive(Debug)]
enum RequestMapping<'i> {
    /// The template the request's content type selects, which for a `MOCK` is what the
    /// integration returns.
    Mapped(&'i str),
    /// No template maps this content type, and the behaviour lets the body through
    /// unmapped — so the call produces no mapped response of its own.
    PassedThrough,
    /// No template maps this content type and the behaviour forbids passing it through.
    Unsupported,
}

/// The integration request template this call selects, by the content type it declares.
///
/// `requestTemplates` is "a key-value map ... the key is an `Accept`/`Content-Type` header
/// value and the value is a mapping template", and `passthroughBehavior` decides a request
/// no key matches: `WHEN_NO_MATCH` "passes the method request body through the integration
/// request without transformation", `WHEN_NO_TEMPLATES` allows that only when the
/// integration declares NO template at all, and `NEVER` "rejects the method request with an
/// HTTP 415 Unsupported Media Type response". A request that declares no content type is
/// read as the JSON one, the type API Gateway assumes for template selection.
fn request_mapping<'i>(
    integration: &'i GetIntegrationOutput,
    call: &MethodRequest,
) -> RequestMapping<'i> {
    let behaviour = integration
        .passthrough_behavior
        .as_deref()
        .unwrap_or(crate::ops::put_integration::DEFAULT_PASSTHROUGH_BEHAVIOR);
    let templates = integration
        .request_templates
        .as_ref()
        .filter(|templates| !templates.is_empty());
    let Some(templates) = templates else {
        // No template at all: only `NEVER` refuses such a call, and the other two
        // behaviours pass the body through.
        return if behaviour == PASSTHROUGH_NEVER {
            RequestMapping::Unsupported
        } else {
            RequestMapping::PassedThrough
        };
    };
    if let Some(template) = templates.get(&call.media_type) {
        return RequestMapping::Mapped(template);
    }
    // Templates exist but none keys this content type: `WHEN_NO_MATCH` is the only
    // behaviour that still lets the body through.
    if behaviour == PASSTHROUGH_WHEN_NO_MATCH {
        RequestMapping::PassedThrough
    } else {
        RequestMapping::Unsupported
    }
}

/// The media type a request declares, as a `requestTemplates` key is compared: the
/// `Content-Type` value with any parameter (`; charset=…`) dropped and the type
/// lowercased, defaulting to [`JSON_CONTENT_TYPE`] when the request declares none.
fn declared_media_type(declared: Option<&str>) -> String {
    declared
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .filter(|media| !media.is_empty())
        .unwrap_or_else(|| JSON_CONTENT_TYPE.to_string())
}

/// The media type a request ACCEPTS, as a `responseTemplates` key is compared: the first
/// entry of the `Accept` value with its parameters dropped, or `None` when the request
/// accepts anything.
fn first_media_type(accepted: Option<&str>) -> Option<String> {
    accepted
        .and_then(|accept| accept.split(',').next())
        .map(|media| media.split(';').next().unwrap_or(media).trim().to_string())
        .filter(|media| !media.is_empty())
}

/// The integration-response status code the selected `MOCK` request template names, or
/// `None` when it names none.
///
/// A `MOCK` integration's request template is where the call's status comes from: the
/// documented form is a literal object carrying the status to return
/// (`{"statusCode": 200}`), and that code is the key of the integration response the call
/// maps to. A template this emulator cannot render — a Velocity one — names no code, and
/// neither does one that is not such an object: the call then maps to the default
/// integration response rather than to a code guessed at.
fn mock_status_code(template: &str) -> Option<String> {
    if is_velocity_template(template) {
        return None;
    }
    let value: ::serde_json::Value = ::serde_json::from_str(template).ok()?;
    let status = value.get("statusCode")?;
    // A JSON number, and the string spelling a template written with quotes produces;
    // both name the same integration response.
    match status {
        ::serde_json::Value::Number(number) => number.as_u64().map(|code| code.to_string()),
        ::serde_json::Value::String(code) => Some(code.clone()),
        _ => None,
    }
}

/// The integration response a `MOCK` call maps to when its request mapping names none:
/// the one with no `selectionPattern`.
///
/// `selectionPattern` is "the regular expression (regex) pattern used to choose an
/// integration response based on the response from the back end" — there is no back-end
/// response here, so a patterned response is never chosen and the default one is.
fn default_integration_response(
    integration: &GetIntegrationOutput,
) -> Option<&GetIntegrationResponseOutput> {
    let responses = integration.integration_responses.as_ref()?;
    let mut defaults: Vec<&GetIntegrationResponseOutput> = responses
        .values()
        .filter(|response| {
            response
                .selection_pattern
                .as_deref()
                .is_none_or(str::is_empty)
        })
        .collect();
    // Keyed by status code in the record, so the choice is stable across calls whatever
    // order the map yields.
    defaults.sort_by(|left, right| left.status_code.cmp(&right.status_code));
    defaults.first().copied()
}

/// A response template this emulator does not render, described for the log: the content
/// type it is keyed by and its size. The template itself is a developer's configuration and
/// is not copied into the log.
#[derive(Debug)]
struct UnrenderableTemplate {
    content_type: String,
    bytes: usize,
}

/// The content type and body the selected integration response renders.
///
/// The template is chosen by the content type the caller asked for (`Accept`), then the
/// JSON one, then the single template an integration response that declares exactly one
/// carries. `Ok` with an EMPTY body when no template applies — a `MOCK` with no template
/// answers no body — and `Err` when rendering the chosen template would mean evaluating
/// Velocity, which this emulator does not do.
fn response_body(
    selected: &GetIntegrationResponseOutput,
    call: &MethodRequest,
) -> Result<(String, Vec<u8>), UnrenderableTemplate> {
    let Some(templates) = selected
        .response_templates
        .as_ref()
        .filter(|templates| !templates.is_empty())
    else {
        return Ok((JSON_CONTENT_TYPE.to_string(), Vec::new()));
    };
    let chosen = call
        .accepted
        .as_deref()
        .and_then(|media| templates.get_key_value(media))
        .or_else(|| templates.get_key_value(JSON_CONTENT_TYPE))
        .or_else(|| {
            let mut entries: Vec<(&String, &String)> = templates.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            (entries.len() == 1).then(|| entries[0])
        });
    let Some((content_type, template)) = chosen else {
        return Ok((JSON_CONTENT_TYPE.to_string(), Vec::new()));
    };
    if is_velocity_template(template) {
        return Err(UnrenderableTemplate {
            content_type: content_type.clone(),
            bytes: template.len(),
        });
    }
    Ok((content_type.clone(), template.clone().into_bytes()))
}

/// The Velocity directives a mapping template may carry, from the Velocity Template
/// Language reference (the `#`-introduced statements of the language a mapping template is
/// written in: "a map of Velocity templates", `requestTemplates`). `#` followed by anything
/// else — `#fff` in a colour, say — is literal text.
const VELOCITY_DIRECTIVES: &[&str] = &[
    "set", "if", "elseif", "else", "end", "foreach", "break", "stop", "macro", "parse",
    "include", "evaluate", "define",
];

/// Whether `template` carries Velocity Template Language rather than being a literal body.
///
/// The language has exactly two constructs: a REFERENCE, `$` (optionally `$!`) followed by
/// an identifier or `{`, and a DIRECTIVE, `#` followed by one of [`VELOCITY_DIRECTIVES`]
/// (optionally braced) or by a comment marker (`##`, `#*`). A template carrying neither
/// needs no evaluation at all: it IS the body, and passing it through is not interpreting a
/// language. One carrying either does need evaluation, and this emulator does not evaluate
/// Velocity, so it is reported as a template that cannot be rendered rather than passed
/// through as if it were literal.
///
/// TOTAL over every `&str`, including a template carrying non-ASCII text: a template is
/// caller-stored data that nothing restricts to ASCII, and this runs while serving a
/// request, so it must not be able to fail. The two bytes it keys on are ASCII, so the
/// position after one is always a character boundary; the remainder is still taken with
/// `str::get`, which answers `None` rather than panicking for any position that is not.
fn is_velocity_template(template: &str) -> bool {
    for (index, byte) in template.as_bytes().iter().enumerate() {
        if *byte != b'$' && *byte != b'#' {
            continue;
        }
        let Some(rest) = template.get(index + 1..) else {
            continue;
        };
        if *byte == b'$' {
            let reference = rest.strip_prefix('!').unwrap_or(rest);
            let identifier = reference.strip_prefix('{').unwrap_or(reference);
            if identifier
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            {
                return true;
            }
            continue;
        }
        if carries_directive_at(rest) {
            return true;
        }
    }
    false
}

/// Whether `template` carries a Velocity DIRECTIVE — the half of the language that is
/// statements rather than references, and the half no substitution can stand in for.
///
/// Split out from [`is_velocity_template`] so the one place that DOES resolve references —
/// a gateway response's body template, whose variables are a documented, closed set
/// ([`gateway_context`]) — can tell "a template naming a variable" from "a template that
/// has to be executed". TOTAL over every `&str` for the reason [`is_velocity_template`]
/// gives.
fn carries_velocity_directive(template: &str) -> bool {
    for (index, byte) in template.as_bytes().iter().enumerate() {
        if *byte != b'#' {
            continue;
        }
        let Some(rest) = template.get(index + 1..) else {
            continue;
        };
        if carries_directive_at(rest) {
            return true;
        }
    }
    false
}

/// Whether the text right after a `#` begins a directive or a comment marker; see
/// [`is_velocity_template`] for why the NAME has to end where an identifier cannot
/// continue.
fn carries_directive_at(rest: &str) -> bool {
    if rest.starts_with('#') || rest.starts_with('*') {
        return true;
    }
    let directive = rest.strip_prefix('{').unwrap_or(rest);
    VELOCITY_DIRECTIVES.iter().any(|name| {
        directive.strip_prefix(name).is_some_and(|after| {
            after
                .chars()
                .next()
                .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '_')
        })
    })
}

/// The `$context` variables a GATEWAY RESPONSE's template names, and what each one stands
/// for in the refusal being answered.
///
/// A gateway response is answered for a request that reached no integration, so the
/// variables it can resolve are the ones describing the REFUSAL. API Gateway's own default
/// template for every type is `{"message":$context.error.messageString}` — the
/// `messageString` form is "the quoted value of `$context.error.message`", quotes
/// included, which is why that template produces a JSON object — beside the message
/// itself, the gateway response type the refusal is answered through, and the validation
/// detail, which is a PLAIN string and which only a refusal that VALIDATED the request
/// has ([`GatewayRefusal::validation_detail`]).
fn gateway_context(refusal: &GatewayRefusal) -> Vec<(&'static str, String)> {
    // Longest path first: `context.error.message` is a PREFIX of
    // `context.error.messageString`, so substituting it first would leave `String` behind.
    vec![
        ("context.error.validationErrorString", refusal.validation_detail.to_string()),
        (
            "context.error.messageString",
            ::serde_json::Value::String(refusal.message.to_string()).to_string(),
        ),
        ("context.error.responseType", refusal.response_type.to_string()),
        ("context.error.message", refusal.message.to_string()),
    ]
}

/// The body a gateway response's `application/json` template renders to for `refusal`, or
/// `None` when this emulator cannot render it.
///
/// The template's `$context` references are resolved from [`gateway_context`], in both
/// spellings the language writes a reference in (`$context.error.message` and
/// `${context.error.message}`); a reference to anything else is left in the body exactly as
/// the template wrote it, which is what the language itself does with a reference it cannot
/// resolve. A template carrying a DIRECTIVE is a template that has to be EXECUTED, which
/// this emulator does not do: it renders nothing rather than a body it guessed at, and the
/// caller gets API Gateway's own default body for the response type.
fn rendered_gateway_template(template: &str, refusal: &GatewayRefusal) -> Option<String> {
    if carries_velocity_directive(template) {
        return None;
    }
    let mut rendered = template.to_string();
    for (path, value) in gateway_context(refusal) {
        rendered = rendered
            .replace(&format!("${{{path}}}"), &value)
            .replace(&format!("${path}"), &value);
    }
    Some(rendered)
}

/// The resource path of the DEPLOYMENT that matches `request_path`, most specific first.
///
/// A literal segment is more specific than a `{parameter}`, which is more specific than a
/// greedy `{proxy+}` — the order API Gateway resolves a request in. Returns `None` when the
/// deployment holds no path that matches.
fn match_resource_path<'d>(
    deployment: &'d DeploymentEntry,
    request_path: &str,
) -> Option<&'d str> {
    deployment
        .api_summary
        .iter()
        .filter(|(_, methods)| !methods.is_empty())
        .filter_map(|(path, _)| match_score(path, request_path).map(|score| (score, path)))
        .max_by(|(left, left_path), (right, right_path)| {
            left.cmp(right).then_with(|| right_path.cmp(left_path))
        })
        .map(|(_, path)| path.as_str())
}

/// How specifically the resource path `template` matches `request_path`, or `None` when it
/// does not match at all.
///
/// One score per template segment, so the comparison is lexicographic: a longer match of
/// equally specific segments wins, and a literal (3) beats a path parameter (2), which
/// beats a greedy parameter (1).
fn match_score(template: &str, request_path: &str) -> Option<Vec<u8>> {
    let template_segments = path_segments(template);
    let request_segments = path_segments(request_path);
    let mut score = Vec::with_capacity(template_segments.len());
    let mut consumed = 0usize;
    for segment in template_segments {
        let parameter = segment
            .strip_prefix('{')
            .and_then(|name| name.strip_suffix('}'));
        match parameter {
            // A greedy parameter consumes every remaining segment, and needs at least one.
            Some(name) if name.ends_with('+') => {
                if consumed >= request_segments.len() {
                    return None;
                }
                score.push(1);
                return Some(score);
            }
            Some(_) => {
                if consumed >= request_segments.len() {
                    return None;
                }
                score.push(2);
                consumed += 1;
            }
            None => {
                if request_segments.get(consumed) != Some(&segment) {
                    return None;
                }
                score.push(3);
                consumed += 1;
            }
        }
    }
    (consumed == request_segments.len()).then_some(score)
}

/// The deployed method of `path` that serves `verb`: the verb's own method, else the `ANY`
/// catch-all.
fn match_method<'d>(
    deployment: &'d DeploymentEntry,
    path: &str,
    verb: &str,
) -> Option<(&'d str, &'d crate::ops::types::MethodSnapshot)> {
    let methods = deployment.api_summary.get(path)?;
    let requested = method_key(verb);
    methods
        .get_key_value(&requested)
        .or_else(|| methods.get_key_value(ANY_METHOD))
        .map(|(verb, snapshot)| (verb.as_str(), snapshot))
}

/// The stage name and the resource path of an invoke path: the first segment is the stage,
/// and what follows is the path within the API (`/` when nothing follows).
///
/// `None` when the path names no stage at all, which is an endpoint that serves nothing.
fn split_stage(path: &str) -> Option<(&str, String)> {
    let segments = path_segments(path);
    let (stage, rest) = segments.split_first()?;
    Some((stage, format!("/{}", rest.join("/"))))
}

/// The non-empty `/`-separated segments of a path.
fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|segment| !segment.is_empty()).collect()
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization` header a
/// signed SDK request carries, or in the `X-Amz-Credential` query parameter of a presigned
/// URL.
///
/// Only PRESENCE is read. Whether the signature over that credential is correct is the
/// emulator's own check, never a service's — which is why this is the predicate BOTH the
/// routing seam and this module use: `Service::override_operation` names the data plane as
/// an operation exactly when this is true, so the router verifies the credential before
/// anything is served, and an `AWS_IAM` method is admitted below exactly when the same
/// predicate holds. Two different predicates would let a presigned invoke of an `AWS_IAM`
/// method be served from the pre-authentication seam.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    if req
        .headers()
        .get("authorization")
        .is_some_and(|value| value.contains("Credential="))
    {
        return true;
    }
    query_of(req.uri())
        .split('&')
        .any(|pair| pair.starts_with("X-Amz-Credential="))
}

/// One gateway-response refusal, rendered as the API serves it on the wire.
///
/// The response NAMES the error shape it is the wire form of, in the header an SDK reads a
/// restJson1 error code out of, for every refusal whose exception this service's model
/// declares ([`GatewayRefusal::error_type`]).
fn refusal_response(api: Option<&RestApiEntry>, refusal: GatewayRefusal) -> RtResponse {
    let error_type = refusal.error_type;
    let (status, body) = refusal_rendering(api, refusal);
    let mut response = response_with(status, JSON_CONTENT_TYPE, body);
    if let Some(error_type) = error_type {
        let _ = response
            .headers_mut()
            .try_insert(ERROR_TYPE_HEADER, error_type);
    }
    response
}

/// The status and body one gateway-response refusal renders to, as the API answers it.
///
/// The status code is the API's customization of this response type when it has one, then
/// its customization of the whole class (`DEFAULT_4XX` / `DEFAULT_5XX`), then the status
/// code the response type itself documents. The body is the customized
/// `application/json` template with its `$context` references resolved
/// ([`rendered_gateway_template`]), and otherwise the default `{"message": …}` body — which
/// is what a template this emulator cannot render falls back to.
///
/// Separate from [`refusal_response`] because the data plane SENDS this rendering while
/// `TestInvokeMethod` REPORTS it as the simulated response, and a developer's
/// customization must reach both.
pub fn refusal_rendering(api: Option<&RestApiEntry>, refusal: GatewayRefusal) -> (u16, Vec<u8>) {
    let documented = crate::state::default_gateway_status_code(refusal.response_type)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(UNPLACEABLE_REFUSAL_STATUS);
    let class_default = if documented < 500 { DEFAULT_4XX } else { DEFAULT_5XX };
    let customized = api.and_then(|api| {
        api.gateway_responses
            .get(refusal.response_type)
            .or_else(|| api.gateway_responses.get(class_default))
    });
    let status = customized
        .and_then(|response| response.status_code.as_deref())
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(documented);
    let templated = customized
        .and_then(|response| response.response_templates.as_ref())
        .and_then(|templates| templates.get(JSON_CONTENT_TYPE))
        .and_then(|template| rendered_gateway_template(template, &refusal));
    let body = match templated {
        Some(template) => template.into_bytes(),
        None => ::serde_json::json!({ refusal.message_member: refusal.message.as_ref() })
            .to_string()
            .into_bytes(),
    };
    (status, body)
}

/// One data-plane response: a status, a content type and a body.
///
/// The data plane is plain HTTPS rather than a modelled operation, so it answers in the
/// envelope API Gateway answers in — a JSON body — instead of this service's restJson1
/// error envelope.
///
/// The content type is a key of a stored `responseTemplates` map, which no operation
/// restricts to a media type, so it is inserted with `try_insert` rather than the panicking
/// `insert`: a value the wire cannot carry is left off the response instead of failing the
/// request.
fn response_with(status: u16, content_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or(
            StatusCode::try_from(UNPLACEABLE_REFUSAL_STATUS).expect("500 is a valid status"),
        ),
        SdkBody::from(body),
    );
    if let Err(refused) = response
        .headers_mut()
        .try_insert("content-type", content_type.to_string())
    {
        tracing::warn!(
            error = %refused,
            "the integration response's content type cannot be sent on the wire and is \
             left off the response"
        );
    }
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

/// The `Host` header of a buffered request, or the URI authority when the request is
/// HTTP/2 (which carries `:authority` on the URI instead of a `Host` header).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn path_of(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

/// The query component of a request URI: everything after the first `?`.
fn query_of(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    //! The two pure rules this module owns that a wire test cannot vary cheaply: which
    //! host is an `execute-api` endpoint, and how specifically a deployed resource path
    //! matches a request path. The served behaviour is pinned end to end in
    //! `tests/all/execute_api_data_plane.rs`.

    use super::*;

    #[test]
    fn an_execute_api_host_is_recognised_by_its_second_label() {
        // Rows differ along the api id, the region, the port, the scheme-level userinfo
        // and the endpoint variant — everything the rule generalises over — plus the hosts
        // that are NOT an api's invoke endpoint.
        let cases: &[(&str, Option<&str>)] = &[
            ("abc1230000.execute-api.us-east-1.amazonaws.com", Some("abc1230000")),
            ("0123456789.execute-api.eu-west-3.amazonaws.com", Some("0123456789")),
            ("a.execute-api.ap-southeast-2.amazonaws.com:443", Some("a")),
            (
                "abc1230000.execute-api-fips.us-gov-west-1.amazonaws.com",
                Some("abc1230000"),
            ),
            ("abc1230000.execute-api.cn-north-1.amazonaws.com.cn", Some("abc1230000")),
            // A label that merely starts with the endpoint's: a different host.
            ("abc1230000.execute-apifoo.us-east-1.amazonaws.com", None),
            // The control plane, whose first label names the service.
            ("apigateway.us-east-1.amazonaws.com", None),
            // The management API's own endpoint host: no api id in front of the label.
            ("execute-api.us-east-1.amazonaws.com", None),
            // Not an endpoint at all.
            ("127.0.0.1:9297", None),
            ("localhost", None),
            ("abc1230000.execute-api", None),
        ];
        for (host, expected) in cases {
            let resolved = api_id_of_host(host);
            assert_eq!(
                resolved, *expected,
                "case {host:?}: expected the api id {expected:?}, got {resolved:?}"
            );
        }
    }

    #[test]
    fn a_vpc_endpoints_own_dns_name_names_the_endpoint_and_not_an_api() {
        // Rows differ along the id width EC2's pattern admits, the zonal suffix an
        // endpoint's per-zone name carries, the region, the port and the endpoint variant —
        // plus the hosts that name NO endpoint, which keep resolving by api id.
        let cases: &[(&str, Option<&str>)] = &[
            (
                "vpce-0123456789abcdef0.execute-api.us-west-2.vpce.amazonaws.com",
                Some("vpce-0123456789abcdef0"),
            ),
            // The zonal name of the SAME endpoint, which EC2 reports beside the regional
            // one: the zone is a suffix of the label, not part of the id.
            (
                "vpce-0123456789abcdef0-us-west-2a.execute-api.us-west-2.vpce.amazonaws.com",
                Some("vpce-0123456789abcdef0"),
            ),
            // AWS's own endpoint-specific public name carries a suffix of its own.
            (
                "vpce-0123456789abcdef0-01234567.execute-api.eu-west-1.vpce.amazonaws.com:443",
                Some("vpce-0123456789abcdef0"),
            ),
            // The eight-digit width the id pattern also admits.
            (
                "vpce-0123abcd.execute-api-fips.us-gov-west-1.vpce.amazonaws.com",
                Some("vpce-0123abcd"),
            ),
            // An api id that merely begins with the prefix is no endpoint id: neither
            // width, and not hexadecimal.
            ("vpce-xyz.execute-api.us-west-2.amazonaws.com", None),
            ("vpcezzzzzzz.execute-api.us-west-2.amazonaws.com", None),
            ("abc1230000.execute-api.us-east-1.amazonaws.com", None),
            // Not an `execute-api` endpoint host at all.
            ("vpce-0123456789abcdef0.s3.us-west-2.vpce.amazonaws.com", None),
            ("localhost", None),
        ];
        for (host, expected) in cases {
            let resolved = endpoint_of_host(host);
            assert_eq!(
                resolved, *expected,
                "case {host:?}: expected the endpoint id {expected:?}, got {resolved:?}"
            );
        }
    }

    #[test]
    fn an_endpoint_host_names_the_region_its_method_arns_carry() {
        // Rows differ along the endpoint form (an api's own, a VPC endpoint's) and the
        // region label, plus a host that names no region at all.
        let cases: &[(Option<&str>, Option<&str>)] = &[
            (Some("abc1230000.execute-api.us-east-1.amazonaws.com"), Some("us-east-1")),
            (
                Some("vpce-0123456789abcdef0.execute-api.eu-west-3.vpce.amazonaws.com"),
                Some("eu-west-3"),
            ),
            (Some("abc1230000.execute-api.ap-southeast-2.amazonaws.com:443"), Some("ap-southeast-2")),
            (Some("127.0.0.1:9297"), None),
            (None, None),
        ];
        for (host, expected) in cases {
            let resolved = region_of_host(*host);
            assert_eq!(
                resolved, *expected,
                "case {host:?}: expected the region {expected:?}, got {resolved:?}"
            );
        }
    }

    #[test]
    fn the_data_plane_path_prefix_names_the_api_before_the_stage() {
        // Rows differ along what follows the prefix (an api id with an invoke path, with a
        // stage alone, with nothing), along the spelling of the prefix itself (extra
        // slashes, a label that merely starts with it, one segment of it alone) and along
        // paths that carry no prefix at all, which the other addressings read.
        let cases: &[(&str, Option<(&str, &str)>)] = &[
            (
                "/_aws/execute-api/abc1230000/prod/pets",
                Some(("abc1230000", "prod/pets")),
            ),
            (
                "/_aws/execute-api/abc1230000/prod",
                Some(("abc1230000", "prod")),
            ),
            ("/_aws/execute-api/abc1230000", Some(("abc1230000", ""))),
            // Empty segments are not segments, so the same address spelled with extra
            // slashes names the same api, and the invoke path it leaves is read by
            // [`split_stage`], which drops them too.
            (
                "//_aws//execute-api//abc1230000//prod//pets",
                Some(("abc1230000", "/prod//pets")),
            ),
            // The prefix is read by exact segment equality, so a segment that merely starts
            // with it is a different path.
            ("/_awsx/execute-api/abc1230000/prod", None),
            ("/_aws/execute-apifoo/abc1230000/prod", None),
            // The prefix with nothing after it names no api.
            ("/_aws/execute-api", None),
            ("/_aws", None),
            // No prefix at all: the api id or the stage leads, for the other addressings.
            ("/abc1230000/prod/pets", None),
            ("/prod/pets", None),
            ("/", None),
            ("", None),
        ];
        for (path, expected) in cases {
            let split = split_execute_api_prefix(path);
            assert_eq!(
                split, *expected,
                "case {path:?}: expected the api id and invoke path {expected:?}, got {split:?}"
            );
        }
    }

    #[test]
    fn a_template_is_velocity_only_when_it_carries_a_reference_or_a_directive() {
        // Rows differ along the construct and along the literal characters that merely
        // LOOK like one: a body that needs no evaluation is served verbatim, so the
        // detector must not claim it does.
        let cases: &[(&str, bool)] = &[
            (r#"{"ok":true}"#, false),
            ("", false),
            // Literal `#` and `$` that introduce nothing.
            (r##"{"color":"#fff"}"##, false),
            (r#"{"price":"$5"}"#, false),
            (r##"{"anchor":"#"}"##, false),
            // Non-ASCII text, which nothing restricts a stored template to: the scan must
            // answer for it rather than fail on a byte index inside a character.
            (r#"{"msg":"café"}"#, false),
            ("☃", false),
            ("🙂 $ 🙂", false),
            (r#"{"msg":"café $input.params('x')"}"#, true),
            ("☃#set($x = 1)", true),
            // References.
            (r#"{"id":"$input.params('id')"}"#, true),
            (r#"{"id":"${input}"}"#, true),
            (r#"{"id":"$!input"}"#, true),
            // Directives and comments.
            ("#set($root = $input.path('$'))", true),
            ("#if($x)a#end", true),
            // A word that merely starts with a directive's name is literal text.
            ("#sets", false),
            ("#ifconfig", false),
            (r##"{"tag":"#end2end"}"##, false),
            ("#end", true),
            ("#set($x = 1)", true),
            ("## a comment", true),
            ("#* a block comment *#", true),
        ];
        for (template, velocity) in cases {
            let detected = is_velocity_template(template);
            assert_eq!(
                detected, *velocity,
                "case {template:?}: expected is_velocity_template to be {velocity}, got \
                 {detected}"
            );
        }
    }

    #[test]
    fn a_literal_segment_beats_a_parameter_which_beats_a_greedy_parameter() {
        // Rows differ along the template's form and the request's segment count: the
        // dimensions the specificity rule generalises over.
        let cases: &[(&str, &str, bool)] = &[
            ("/", "/", true),
            ("/pets", "/pets", true),
            ("/pets", "/pets/1", false),
            ("/pets/{petId}", "/pets/1", true),
            ("/pets/{petId}", "/pets", false),
            ("/pets/{petId}", "/pets/1/toys", false),
            ("/{proxy+}", "/pets/1/toys", true),
            ("/{proxy+}", "/", false),
            ("/pets/{proxy+}", "/pets/1/toys", true),
        ];
        for (template, request, matches) in cases {
            let scored = match_score(template, request).is_some();
            assert_eq!(
                scored, *matches,
                "case {template:?} vs {request:?}: expected a match of {matches}, got {scored}"
            );
        }
        let literal = match_score("/pets/mine", "/pets/mine").expect("the literal matches");
        let parameter = match_score("/pets/{petId}", "/pets/mine").expect("the parameter matches");
        let greedy = match_score("/{proxy+}", "/pets/mine").expect("the greedy parameter matches");
        assert!(
            literal > parameter && parameter > greedy,
            "case specificity: expected literal > parameter > greedy, got \
             {literal:?} > {parameter:?} > {greedy:?}"
        );
    }
}
