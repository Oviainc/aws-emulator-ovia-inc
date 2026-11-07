// SPDX-License-Identifier: CC0-1.0

//! The `AWS_PROXY` integration: how a method request is served by INVOKING the Lambda
//! function its `uri` names, in process, and answering what the handler returned.
//!
//! `IntegrationType` gives `AWS_PROXY` "for integrating the API method request with the
//! Lambda function-invoking action with the client request passed through as-is", and
//! `Integration$uri` gives the form that action is named in: "for integrations of the
//! `aws` type … `arn:aws:apigateway:{region}:{subdomain.service|service}:path|action/\
//! {service_api}`", which for the function-invoking action is the Lambda data-plane path
//! `path/{version}/functions/{function-arn}/invocations`. So the `uri` names ONE function,
//! and the call is made against it THROUGH THIS EMULATOR, over the in-process transport
//! `OperationContext::callback` hands a handler: no connection is opened, and what the
//! method request is answered with is what the function's own run produced.
//!
//! # What is passed through, and what comes back
//!
//! A proxy integration maps nothing: "the client request [is] passed through as-is". API
//! Gateway hands the function ONE event document describing the whole method request — the
//! resource template it matched and the path it arrived at, the verb, the headers and query
//! string in both their single-valued and multi-valued forms, the path parameters the
//! template bound, the stage's variables, the call's context, and the body — and takes the
//! function's returned document as the response: `statusCode`, `headers`,
//! `multiValueHeaders`, `body` and `isBase64Encoded`. Neither direction consults an
//! integration request template, an integration response or a method response: a proxy
//! integration has none to consult, which is what makes it a proxy.
//!
//! Every value in the event is read off THIS call and this api's own records. Nothing is
//! invented: a member whose value this emulator does not have is left out rather than
//! filled in, and the body travels exactly as it arrived — as text where the request's bytes
//! are text, and base64-encoded with `isBase64Encoded` set where they are not, which is the
//! encoding the member documents.
//!
//! # Who the call is made as, and what refuses it
//!
//! API Gateway invokes a function as its own service principal ([`SERVICE_PRINCIPAL`]), so
//! the grant that admits the invocation is the FUNCTION's resource-based policy — the
//! document `AddPermission` writes and `GetPolicy` reports — and it is read from Lambda
//! itself rather than kept a second time here. The decision is
//! [`::service_common::iam_authorization`], the tree's one evaluation of policy documents,
//! taken on `lambda:InvokeFunction` over the function the `uri` names with the invoking
//! METHOD's ARN as the `aws:SourceArn` such a grant is narrowed by — which is how a
//! permission added for one api, stage or path admits a call to it and no other.
//!
//! Nothing is refused on evidence this emulator does not have: a `uri` whose function Lambda
//! does not answer about at all, an emulator with no Lambda mounted, and an invocation that
//! reached no stage (a simulated one, which has no method ARN) each leave the call to be
//! made, for the reason [`super::super::resource_policy`]'s own reading gives. A function
//! whose policy Lambda REPORTS as absent is an answer rather than a silence: it grants
//! nothing, so the call is not made.
//!
//! # How an outcome becomes the method response
//!
//! * the function RAN and returned a well-formed proxy response: that response IS the method
//!   response — its status, its headers and its body, unmapped;
//! * the function ran and returned something else, or raised (the `FunctionError` signal
//!   Lambda reports): API Gateway "returns a 502 Bad Gateway" for a proxy response not in
//!   the required format, with the `{"message":"Internal server error"}` body it words such
//!   a response with, and the reason is logged ([`BAD_GATEWAY_STATUS`]);
//! * the invocation could not be MADE at all — no Lambda answered it, the `uri` names no
//!   function Lambda holds, or the function's policy admits no call from this method: there
//!   is no function answer to pass through, so the call is answered the way every integration
//!   this stage cannot perform is, through the api's own `API_CONFIGURATION_ERROR` gateway
//!   response ([`super::API_CONFIGURATION_ERROR`], `500` with
//!   `{"message":"Internal server error"}`).

use std::collections::BTreeMap;
use std::time::SystemTime;

use ::base64::Engine as _;
use ::serde_json::{json, Map, Value};
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::OperationContext;

use crate::ops::put_integration::LAMBDA_PROXY_INTEGRATION_TYPE;

use super::{IntegratedResponse, MethodRequest, UnservedIntegration, API_CONFIGURATION_ERROR,
            JSON_CONTENT_TYPE};

/// The vendor field of every AWS integration ARN, from the form `Integration$uri`
/// documents: `arn:aws:apigateway:{region}:{subdomain}:path|action/…`.
const APIGATEWAY_VENDOR: &str = "apigateway";

/// The subdomain field that names AWS Lambda, which is the service whose function-invoking
/// action this module performs.
const LAMBDA_SUBDOMAIN: &str = "lambda";

/// The resource prefix of the PATH form: the rest of the resource is the service's own REST
/// path, which for the function-invoking action is
/// `{version}/functions/{function-arn}/invocations`.
const PATH_FORM_PREFIX: &str = "path/";

/// The segment of that path that the function's own name or ARN follows.
const FUNCTIONS_SEGMENT: &str = "/functions/";

/// The last segment of that path: Lambda's own synchronous-invocation resource.
const INVOCATIONS_SUFFIX: &str = "/invocations";

/// The service principal API Gateway invokes a function as, as a resource policy names one:
/// "for Amazon Web Services services, the principal is a domain-style identifier that the
/// service defines" (`AddPermission$Principal`).
const SERVICE_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The IAM action an invocation of a function performs, which is not the operation's own
/// name: `Invoke` "requires permission for the `lambda:InvokeFunction` action".
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The condition key a grant narrowed to ONE invoking resource names the source by, which
/// for an api's method is that method's `execute-api` ARN.
const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The condition key a grant narrowed to the account that OWNS the invoking api names it by.
const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The status API Gateway answers a proxy integration whose function did not return a
/// well-formed proxy response with: "If the response is not in this format, API Gateway
/// returns a 502 Bad Gateway error response."
const BAD_GATEWAY_STATUS: u16 = 502;

/// The sentence that response carries, which is the one API Gateway words every
/// server-side failure of a call it could not pass on with.
const BAD_GATEWAY_MESSAGE: &str = "Internal server error";

/// The member the function's returned document carries the response status in.
const STATUS_CODE_MEMBER: &str = "statusCode";

/// The member it carries single-valued response headers in.
const HEADERS_MEMBER: &str = "headers";

/// The member it carries multi-valued response headers in.
const MULTI_VALUE_HEADERS_MEMBER: &str = "multiValueHeaders";

/// The member it carries the response body in, always as text.
const BODY_MEMBER: &str = "body";

/// The member that says the body is base64-encoded, in both directions.
const BASE64_MEMBER: &str = "isBase64Encoded";

/// The response header whose value is the method response's media type rather than one of
/// its headers.
const CONTENT_TYPE_HEADER: &str = "content-type";

/// The `$context` member path an authorizer's claims are published under, which the event
/// carries as `requestContext.authorizer.claims`.
const AUTHORIZER_CLAIMS_MEMBER: &str = "authorizer.claims";

/// The request header a proxy integration reads the caller's address off where the request
/// reached the emulator through a forwarder; a request carrying none names no address, so
/// the event carries none either.
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The request header the caller's user agent travels in.
const USER_AGENT_HEADER: &str = "user-agent";

/// The request headers the event does NOT carry over from the caller's request: the two the
/// caller's own SigV4 CREDENTIAL travels in.
///
/// The same withholding the pass-through path makes, for the same reason
/// ([`super::HEADERS_NOT_PASSED_THROUGH`]): a credential the client presented to this
/// endpoint authenticates nothing at the backend the integration names, and handing it to
/// code the api's developer deployed would make every invoke a way to collect the caller's
/// credential. What API Gateway's own integration credential is is
/// `Integration$credentials` — a record of the integration — and the invocation here is made
/// under the transport's, never the caller's. Every OTHER header the caller sent travels in
/// the event, which is what "the client request passed through as-is" means.
const HEADERS_WITHHELD_FROM_THE_EVENT: &[&str] = &["authorization", "x-amz-security-token"];

/// The most bytes one function answer may carry into a method response: API Gateway's
/// published payload quota is 10 MB, so an answer past it is one this stage cannot pass on
/// rather than one buffered without bound.
const MAX_PROXY_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;

/// What a PROXY integration's event carries about the call beyond what a mapping template
/// reads off it.
///
/// A proxy integration passes "the client request … through as-is", so the event names parts
/// of the request a template has no reference for: every value a repeated query parameter or
/// header carries, the verb as the CLIENT sent it (rather than the `ANY` catch-all it may
/// have matched), the `Resource` record the path resolved to, and the endpoint it arrived on.
/// Built by whatever is serving the call — the data plane off the wire, `TestInvokeMethod`
/// out of the members a caller simulates a request with — so one function sees one event
/// shape whichever side drives it.
#[derive(Debug, Default, Clone)]
pub struct ProxyInputs {
    /// The id of the `Resource` whose path the request matched.
    pub resource_id: String,
    /// The verb the CLIENT sent, which is what the event reports as the request's method.
    pub http_method: String,
    /// The path the request carried WITHIN the stage, leading `/` included.
    pub request_path: String,
    /// The endpoint host the request was addressed on, or empty where it named none.
    pub domain_name: String,
    /// Every value the request carries under each query-string name, in the order sent.
    pub multi_value_querystring: BTreeMap<String, Vec<String>>,
    /// Every value the request carries under each header name, in the order sent.
    pub multi_value_header: BTreeMap<String, Vec<String>>,
    /// The ARN of the METHOD this call addresses — the `aws:SourceArn` an `AddPermission`
    /// grant to API Gateway is narrowed by — or `None` for a call that reached no stage,
    /// which names no method for a grant to be judged against.
    pub source_arn: Option<String>,
}

/// Perform this call through the method's `AWS_PROXY` integration, or `None` when the
/// integration is not a Lambda proxy — in which case the caller performs it from the api's
/// own records ([`super::perform_integration`]).
///
/// `region` is the api's own Region, which is the endpoint the invocation is made at when the
/// integration ARN names none of its own.
pub(super) async fn performed(
    ctx: &OperationContext<'_>,
    region: &str,
    method: &crate::state::MethodEntry,
    call: &MethodRequest,
) -> Option<Result<IntegratedResponse, UnservedIntegration>> {
    let integration = method.integration.as_ref()?;
    if integration.r#type.as_deref() != Some(LAMBDA_PROXY_INTEGRATION_TYPE) {
        return None;
    }
    let uri = integration.uri.as_deref().unwrap_or_default();
    let Some((addressed_region, function)) = invoked_function(uri) else {
        return Some(Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the AWS_PROXY integration's uri does not name a Lambda function's \
                     invocation path, so there is no function to invoke"
                .to_string(),
        }));
    };
    let at = if addressed_region.is_empty() {
        region.to_string()
    } else {
        addressed_region.to_string()
    };
    Some(served(ctx, &at, function, call).await)
}

/// The two parts of an `AWS_PROXY` `uri` that decide the invocation: the Region of the
/// Lambda endpoint it names (empty where it names none) and the function it addresses.
///
/// `None` for a value that is not the function-invoking action's path at all, which is an
/// integration this call cannot be performed through rather than an address guessed at.
fn invoked_function(uri: &str) -> Option<(&str, &str)> {
    // Six fields, because the SIXTH — the resource — carries the function's own ARN, whose
    // colons must stay where they are.
    let mut fields = uri.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next()? != APIGATEWAY_VENDOR {
        return None;
    }
    let region = fields.next()?;
    if !fields.next()?.eq_ignore_ascii_case(LAMBDA_SUBDOMAIN) {
        return None;
    }
    let path = fields.next()?.strip_prefix(PATH_FORM_PREFIX)?;
    let invoked = path.strip_suffix(INVOCATIONS_SUFFIX)?;
    let (_api_version, function) = invoked.split_once(FUNCTIONS_SEGMENT)?;
    (!function.is_empty()).then_some((region, function))
}

/// The method response this call earns: the function's policy decides whether the
/// invocation is made, the run decides what it answered, and that answer IS the response.
async fn served(
    ctx: &OperationContext<'_>,
    region: &str,
    function: &str,
    call: &MethodRequest,
) -> Result<IntegratedResponse, UnservedIntegration> {
    let client = lambda_client(ctx, region);
    if let Some(reason) = unpermitted(&client, function, call).await {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason,
        });
    }
    let event = proxy_event(call);
    let payload = ::serde_json::to_vec(&event).map_err(|_| UnservedIntegration {
        refusal: API_CONFIGURATION_ERROR,
        reason: "the proxy integration's event could not be written, so no invocation was \
                 made"
            .to_string(),
    })?;
    bounded(payload.len())?;
    let answer = client
        .invoke()
        .function_name(function)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(payload))
        .send()
        .await;
    let answered = match answer {
        Ok(answered) => answered,
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the proxy integration's function was not invoked"
            );
            return Err(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason: "the AWS_PROXY integration's function did not answer the \
                         invocation, so there is no function answer to pass through"
                    .to_string(),
            });
        }
    };
    // "FunctionError: if present, indicates that an error occurred during function
    // execution" — a run that errored returned no proxy response, which API Gateway answers
    // the same way it answers one it cannot read.
    if let Some(signal) = answered.function_error() {
        ::tracing::warn!(
            function_error = %signal,
            "the proxy integration's function errored, so it returned no proxy response"
        );
        return Ok(bad_gateway());
    }
    let returned = answered
        .payload()
        .map(|payload| payload.as_ref().to_vec())
        .unwrap_or_default();
    bounded(returned.len())?;
    match proxy_response(&returned) {
        Ok(response) => Ok(response),
        Err(reason) => {
            ::tracing::warn!(
                reason = %reason,
                "the proxy integration's function returned no well-formed proxy response"
            );
            Ok(bad_gateway())
        }
    }
}

/// `Ok` while `bytes` is within what one proxy payload may carry
/// ([`MAX_PROXY_PAYLOAD_BYTES`]).
fn bounded(bytes: usize) -> Result<(), UnservedIntegration> {
    if bytes > MAX_PROXY_PAYLOAD_BYTES {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the proxy integration's payload of {bytes} bytes is larger than the \
                 {MAX_PROXY_PAYLOAD_BYTES} bytes one invocation carries"
            ),
        });
    }
    Ok(())
}

/// The response API Gateway answers a proxy integration whose function returned no
/// well-formed proxy response with.
fn bad_gateway() -> IntegratedResponse {
    IntegratedResponse {
        status: BAD_GATEWAY_STATUS,
        content_type: JSON_CONTENT_TYPE.to_string(),
        body: json!({ "message": BAD_GATEWAY_MESSAGE }).to_string().into_bytes(),
        headers: BTreeMap::new(),
    }
}

/// Why the function's own resource policy does not admit this invocation, or `None` when it
/// does — and when nothing about it could be shown, which refuses nothing.
async fn unpermitted(
    client: &::aws_sdk_lambda::Client,
    function: &str,
    call: &MethodRequest,
) -> Option<String> {
    // A call that reached no stage names no method for a grant's `aws:SourceArn` to be
    // judged against, so there is no decision to take about it.
    let source_arn = call.proxy.source_arn.as_deref()?;
    // The RESOURCE a grant is taken on is the function's own ARN, whatever spelling the
    // integration's `uri` addressed it by, so it is resolved from Lambda rather than assumed
    // to be what the `uri` carried. A function Lambda answers nothing about is one this
    // module judges nothing about: the invocation below is what then decides the call.
    let resolved = match client
        .get_function_configuration()
        .function_name(function)
        .send()
        .await
    {
        Ok(reported) => reported.function_arn.unwrap_or_else(|| function.to_string()),
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the proxy integration's function was not judged: nothing answered for its \
                 configuration"
            );
            return None;
        }
    };
    let answer = client.get_policy().function_name(&resolved).send().await;
    let document = match answer {
        Ok(reported) => reported.policy.unwrap_or_default(),
        Err(error) => {
            if !refused_by_lambda(&error) {
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the proxy integration's function was not judged: nothing answered for \
                     its resource policy"
                );
                return None;
            }
            // A function Lambda reports no resource policy for carries none, which grants
            // nothing.
            String::new()
        }
    };
    let principals = [SERVICE_PRINCIPAL.to_string()];
    let context = [
        (SOURCE_ARN_KEY.to_string(), source_arn.to_string()),
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::emulator_account_id().to_string(),
        ),
    ];
    let request = iam::Request::new(INVOKE_FUNCTION_ACTION, &resolved, &principals)
        .with_context(&context)
        .with_published_keys(&[SOURCE_ARN_KEY, SOURCE_ACCOUNT_KEY]);
    if iam::resource_decision(&document, &request) == Decision::Allowed {
        return None;
    }
    Some(format!(
        "the AWS_PROXY integration's function allows {SERVICE_PRINCIPAL} no \
         {INVOKE_FUNCTION_ACTION} for this method, so no invocation was made"
    ))
}

/// Whether `error` is LAMBDA refusing the call rather than the emulator failing to carry it.
///
/// A failure the emulator itself produced — no service of it serves the endpoint, or more
/// than one claims it — is not Lambda answering, so it grounds no refusal: `service_common`
/// owns the one reading of that refusal.
fn refused_by_lambda<E>(
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        E,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    let ::aws_smithy_runtime_api::client::result::SdkError::ServiceError(service) = error else {
        return false;
    };
    let body = service.raw().body().bytes().unwrap_or_default();
    !::service_common::in_process_refusal::is_transport_refusal(body)
}

/// The Lambda client the invocation is made with.
///
/// The transport is the emulator's own, in process, so the call is served by the Lambda this
/// emulator mounts and no connection is opened; the credentials are the ones the callback
/// hands over, which sign this call and are never reported or logged.
fn lambda_client(ctx: &OperationContext<'_>, region: &str) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given.
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

/// The event document the function is handed for this call.
///
/// Every member is read off the call and this api's own records. A map the call has no
/// entries for is `null` rather than empty, which is the value API Gateway sends for a
/// request carrying no query string, no path parameter and no stage variable.
fn proxy_event(call: &MethodRequest) -> Value {
    let inputs = &call.inputs;
    let proxy = &call.proxy;
    let context = &inputs.context;
    let stage = context.get("stage").cloned().unwrap_or_default();
    let (body, base64_encoded) = event_body(inputs.payload.as_deref());
    let mut event = Map::new();
    event.insert(
        "resource".to_string(),
        json!(context.get("resourcePath").cloned().unwrap_or_default()),
    );
    event.insert("path".to_string(), json!(proxy.request_path));
    event.insert("httpMethod".to_string(), json!(proxy.http_method));
    event.insert(
        "headers".to_string(),
        single_valued(&withheld(&inputs.header)),
    );
    event.insert(
        MULTI_VALUE_HEADERS_MEMBER.to_string(),
        multi_valued(&withheld(&proxy.multi_value_header)),
    );
    event.insert(
        "queryStringParameters".to_string(),
        or_null(single_valued(&inputs.querystring)),
    );
    event.insert(
        "multiValueQueryStringParameters".to_string(),
        or_null(multi_valued(&proxy.multi_value_querystring)),
    );
    event.insert(
        "pathParameters".to_string(),
        or_null(single_valued(&inputs.path)),
    );
    event.insert(
        "stageVariables".to_string(),
        or_null(single_valued(&inputs.stage_variables)),
    );
    event.insert(
        "requestContext".to_string(),
        request_context(call, &stage),
    );
    event.insert(BODY_MEMBER.to_string(), body);
    event.insert(BASE64_MEMBER.to_string(), json!(base64_encoded));
    Value::Object(event)
}

/// The `requestContext` of the event: what this api, this stage and this request are, each
/// read off the records that resolved the call.
fn request_context(call: &MethodRequest, stage: &str) -> Value {
    let context = &call.inputs.context;
    let proxy = &call.proxy;
    let now = SystemTime::now();
    let mut reported = Map::new();
    reported.insert(
        "apiId".to_string(),
        json!(context.get("apiId").cloned().unwrap_or_default()),
    );
    reported.insert("stage".to_string(), json!(stage));
    reported.insert("resourceId".to_string(), json!(proxy.resource_id));
    reported.insert(
        "resourcePath".to_string(),
        json!(context.get("resourcePath").cloned().unwrap_or_default()),
    );
    reported.insert("httpMethod".to_string(), json!(proxy.http_method));
    // The path API Gateway reports is the one the api's own endpoint carries: the stage and
    // then the resource path, whichever addressing the client reached this emulator by.
    reported.insert(
        "path".to_string(),
        json!(format!("/{stage}{}", proxy.request_path)),
    );
    reported.insert(
        "accountId".to_string(),
        json!(::engine::emulator_account_id()),
    );
    if !proxy.domain_name.is_empty() {
        reported.insert("domainName".to_string(), json!(proxy.domain_name));
        if let Some((prefix, _)) = proxy.domain_name.split_once('.') {
            reported.insert("domainPrefix".to_string(), json!(prefix));
        }
    }
    // The request id is minted where it is reported, as every other response of this data
    // plane reports one: `service_common` owns the minting.
    let request_id = ::service_common::query_error_envelope::new_request_id();
    reported.insert("requestId".to_string(), json!(request_id.clone()));
    reported.insert("extendedRequestId".to_string(), json!(request_id));
    if let Some(clf) = common_log_time(now) {
        reported.insert("requestTime".to_string(), json!(clf));
    }
    reported.insert(
        "requestTimeEpoch".to_string(),
        json!(epoch_millis(now)),
    );
    reported.insert("identity".to_string(), identity(call));
    // The claims the method's authorizer published for this call, under the member path a
    // proxy event carries them in; a method whose authorization publishes none adds none.
    let claims: Map<String, Value> = context
        .iter()
        .filter_map(|(member, value)| {
            member
                .strip_prefix(&format!("{AUTHORIZER_CLAIMS_MEMBER}."))
                .map(|claim| (claim.to_string(), json!(value)))
        })
        .collect();
    if !claims.is_empty() {
        reported.insert(
            "authorizer".to_string(),
            json!({ "claims": Value::Object(claims) }),
        );
    }
    Value::Object(reported)
}

/// The `identity` of the event: what the REQUEST says about who made it, and nothing else.
///
/// Only the two values a request off the wire carries are reported — the address a forwarder
/// named it by and the user agent it declared — because every other member of this object
/// names a principal API Gateway authenticated, and a method is authenticated by its own
/// `authorizationType` rather than by this object.
fn identity(call: &MethodRequest) -> Value {
    let mut reported = Map::new();
    if let Some(address) = header_of(call, FORWARDED_FOR_HEADER)
        .and_then(|forwarded| forwarded.split(',').next().map(str::trim).map(str::to_string))
        .filter(|address| !address.is_empty())
    {
        reported.insert("sourceIp".to_string(), json!(address));
    }
    if let Some(agent) = header_of(call, USER_AGENT_HEADER) {
        reported.insert("userAgent".to_string(), json!(agent));
    }
    Value::Object(reported)
}

/// The value the call carries for the request header `name`, looked up case-insensitively
/// because a header name is.
fn header_of(call: &MethodRequest, name: &str) -> Option<String> {
    call.inputs
        .header
        .iter()
        .find(|(sent, _)| sent.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// The event's `body` and `isBase64Encoded`: the request's own bytes as TEXT where they are
/// text, and base64-encoded where they are not — "the body is base64-encoded" is exactly
/// what that member says.
///
/// A call carrying no payload carries no body, which the event reports as `null` rather than
/// as the empty string: API Gateway sends `null` for a request with no body at all.
fn event_body(payload: Option<&[u8]>) -> (Value, bool) {
    let Some(payload) = payload.filter(|bytes| !bytes.is_empty()) else {
        return (Value::Null, false);
    };
    match ::std::str::from_utf8(payload) {
        Ok(text) => (json!(text), false),
        Err(_) => (
            json!(::base64::engine::general_purpose::STANDARD.encode(payload)),
            true,
        ),
    }
}

/// One single-valued map of the call as a JSON object.
fn single_valued(values: &BTreeMap<String, String>) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(name, value)| (name.clone(), json!(value)))
            .collect(),
    )
}

/// `headers` without the names [`HEADERS_WITHHELD_FROM_THE_EVENT`] lists, compared
/// case-insensitively because a header name is.
fn withheld<V: Clone>(headers: &BTreeMap<String, V>) -> BTreeMap<String, V> {
    headers
        .iter()
        .filter(|(name, _)| {
            !HEADERS_WITHHELD_FROM_THE_EVENT
                .iter()
                .any(|withheld| name.eq_ignore_ascii_case(withheld))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// One multi-valued map of the call as a JSON object of arrays.
fn multi_valued(values: &BTreeMap<String, Vec<String>>) -> Value {
    Value::Object(
        values
            .iter()
            .map(|(name, carried)| (name.clone(), json!(carried)))
            .collect(),
    )
}

/// `Null` for an empty object, which is the value API Gateway sends for a map the request
/// has no entries for.
fn or_null(object: Value) -> Value {
    match &object {
        Value::Object(entries) if entries.is_empty() => Value::Null,
        _ => object,
    }
}

/// The instant `at` in the Common Log Format API Gateway reports `requestTime` in
/// (`dd/Mon/yyyy:HH:mm:ss +0000`), built from the one HTTP-date rendering this tree has, so
/// there is no second calendar here.
fn common_log_time(at: SystemTime) -> Option<String> {
    // The HTTP date is `Wed, 21 Oct 2015 07:28:00 GMT`: the same fields, in the same units.
    let http_date = ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::HttpDate)
        .ok()?;
    let (_weekday, rest) = http_date.split_once(", ")?;
    let mut fields = rest.split(' ');
    let day = fields.next()?;
    let month = fields.next()?;
    let year = fields.next()?;
    let time = fields.next()?;
    Some(format!("{day}/{month}/{year}:{time} +0000"))
}

/// The instant `at` in milliseconds since the epoch, which is the unit `requestTimeEpoch`
/// reports.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_millis()).ok())
        .unwrap_or_default()
}

/// The method response the function's returned document IS, or the reason it is not a
/// well-formed proxy response.
///
/// A proxy integration maps nothing, so every part of the response is read straight out of
/// the document: the status it names, the headers it declares in either member, and the body
/// it carries — decoded from base64 where it says it is encoded.
fn proxy_response(returned: &[u8]) -> Result<IntegratedResponse, String> {
    let Ok(Value::Object(document)) = ::serde_json::from_slice::<Value>(returned) else {
        return Err("the function returned no JSON object".to_string());
    };
    let status = status_code(document.get(STATUS_CODE_MEMBER))
        .ok_or_else(|| format!("the function's {STATUS_CODE_MEMBER} is not an HTTP status"))?;

    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in object_of(document.get(HEADERS_MEMBER), HEADERS_MEMBER)? {
        let text = header_value(value)
            .ok_or_else(|| format!("the function's {HEADERS_MEMBER}.{name} is not a value a \
                                    header carries"))?;
        headers.insert(name.clone(), vec![text]);
    }
    for (name, value) in object_of(
        document.get(MULTI_VALUE_HEADERS_MEMBER),
        MULTI_VALUE_HEADERS_MEMBER,
    )? {
        let Value::Array(carried) = value else {
            return Err(format!(
                "the function's {MULTI_VALUE_HEADERS_MEMBER}.{name} is not a list of values"
            ));
        };
        let mut values = Vec::with_capacity(carried.len());
        for one in carried {
            values.push(header_value(one).ok_or_else(|| {
                format!(
                    "the function's {MULTI_VALUE_HEADERS_MEMBER}.{name} carries a value no \
                     header carries"
                )
            })?);
        }
        // The multi-valued member is the complete set of values for the names it declares,
        // so it REPLACES what the single-valued member said about the same name.
        headers.insert(name.clone(), values);
    }

    let base64_encoded = match document.get(BASE64_MEMBER) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(encoded)) => *encoded,
        Some(_) => return Err(format!("the function's {BASE64_MEMBER} is not a boolean")),
    };
    let body = match document.get(BODY_MEMBER) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) if base64_encoded => {
            ::base64::engine::general_purpose::STANDARD
                .decode(text)
                .map_err(|_| {
                    format!("the function's {BODY_MEMBER} is not the base64 its \
                             {BASE64_MEMBER} declares")
                })?
        }
        Some(Value::String(text)) => text.clone().into_bytes(),
        Some(_) => return Err(format!("the function's {BODY_MEMBER} is not a string")),
    };

    // The media type is one of the headers the function declared, and `application/json` is
    // what API Gateway sends for a proxy response that declares none.
    let declared = headers
        .keys()
        .find(|name| name.eq_ignore_ascii_case(CONTENT_TYPE_HEADER))
        .cloned();
    let content_type = declared
        .and_then(|name| {
            headers
                .remove(&name)
                .and_then(|values| values.into_iter().next())
        })
        .unwrap_or_else(|| JSON_CONTENT_TYPE.to_string());
    Ok(IntegratedResponse {
        status,
        content_type,
        body,
        headers,
    })
}

/// The status the function's `statusCode` names, which the member carries as a number or as
/// the text of one, and which has to be an HTTP status code.
fn status_code(carried: Option<&Value>) -> Option<u16> {
    let status = match carried? {
        Value::Number(number) => u16::try_from(number.as_u64()?).ok()?,
        Value::String(text) => text.trim().parse::<u16>().ok()?,
        _ => return None,
    };
    (100..=599).contains(&status).then_some(status)
}

/// The entries of an OBJECT member of the returned document, empty for one it does not
/// carry, or the reason the member is not an object at all.
fn object_of<'d>(
    carried: Option<&'d Value>,
    member: &str,
) -> Result<Vec<(&'d String, &'d Value)>, String> {
    match carried {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(entries)) => Ok(entries.iter().collect()),
        Some(_) => Err(format!("the function's {member} is not an object")),
    }
}

/// The text one header value is, for the three JSON scalars a proxy response may declare
/// one as; `None` for a value no header carries.
fn header_value(carried: &Value) -> Option<String> {
    match carried {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `uri` row: the case, the value, and the Region and function it names (`None` for
    /// a value that names no function-invoking action at all).
    struct UriRow {
        case: &'static str,
        uri: String,
        names: Option<(String, String)>,
    }

    /// The `uri` form the function-invoking action is named in, over the parts that decide
    /// the invocation — and the forms that name no function at all.
    #[test]
    fn an_aws_proxy_uri_names_one_function_and_the_region_it_is_invoked_in() {
        let function = "arn:aws:lambda:us-east-1:000000000000:function:writer";
        let qualified = format!("{function}:live");
        let rows = [
            UriRow {
                case: "the documented form, naming the function by ARN",
                uri: format!(
                    "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
                     {function}/invocations"
                ),
                names: Some(("us-east-1".to_string(), function.to_string())),
            },
            UriRow {
                case: "an alias qualifier, which is part of the function the uri addresses",
                uri: format!(
                    "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/\
                     {qualified}/invocations"
                ),
                names: Some(("us-east-1".to_string(), qualified.clone())),
            },
            UriRow {
                case: "the function named by name alone, at another Region's endpoint",
                uri: "arn:aws:apigateway:eu-west-1:lambda:path/2015-03-31/functions/writer\
                      /invocations"
                    .to_string(),
                names: Some(("eu-west-1".to_string(), "writer".to_string())),
            },
            UriRow {
                case: "no invocations segment",
                uri: format!(
                    "arn:aws:apigateway:us-east-1:lambda:path/2015-03-31/functions/{function}"
                ),
                names: None,
            },
            UriRow {
                case: "the action form, which names no invocation path",
                uri: "arn:aws:apigateway:us-east-1:lambda:action/Invoke".to_string(),
                names: None,
            },
            UriRow {
                case: "another service's subdomain",
                uri: "arn:aws:apigateway:us-east-1:s3:path/2015-03-31/functions/writer\
                      /invocations"
                    .to_string(),
                names: None,
            },
            UriRow {
                case: "a uri that is no ARN at all",
                uri: "https://lambda.us-east-1.amazonaws.com/2015-03-31/functions/writer\
                      /invocations"
                    .to_string(),
                names: None,
            },
        ];
        for row in &rows {
            let read = invoked_function(&row.uri).map(|(region, function)| {
                (region.to_string(), function.to_string())
            });
            assert_eq!(
                read, row.names,
                "case {:?}: expected {:?}, got {read:?}",
                row.case, row.names
            );
        }
    }

    /// What a function's returned document maps to, over the shapes a proxy response takes
    /// and the ones that are not proxy responses at all.
    #[test]
    fn a_proxy_response_is_the_document_the_function_returned() {
        let served = proxy_response(
            br#"{"statusCode":201,"headers":{"Content-Type":"text/plain","X-One":"1"},
                 "multiValueHeaders":{"Set-Cookie":["a=1","b=2"]},"body":"hello"}"#,
        )
        .expect("case \"a well-formed proxy response\": expected it to be read");
        assert_eq!(
            served.status, 201,
            "case \"a well-formed proxy response\": expected 201, got {}",
            served.status
        );
        assert_eq!(
            served.content_type, "text/plain",
            "case \"a well-formed proxy response\": expected the declared media type, got {:?}",
            served.content_type
        );
        assert_eq!(
            served.body, b"hello",
            "case \"a well-formed proxy response\": expected the declared body, got {:?}",
            String::from_utf8_lossy(&served.body)
        );
        assert_eq!(
            served.headers.get("Set-Cookie").map(Vec::as_slice),
            Some(["a=1".to_string(), "b=2".to_string()].as_slice()),
            "case \"a well-formed proxy response\": expected both cookie values, got {:?}",
            served.headers
        );

        let encoded = proxy_response(
            br#"{"statusCode":"200","body":"AAEC","isBase64Encoded":true}"#,
        )
        .expect("case \"a base64 body\": expected it to be read");
        assert_eq!(
            encoded.body,
            vec![0u8, 1, 2],
            "case \"a base64 body\": expected the decoded bytes, got {:?}",
            encoded.body
        );
        assert_eq!(
            encoded.content_type, JSON_CONTENT_TYPE,
            "case \"a base64 body\": expected the default media type, got {:?}",
            encoded.content_type
        );

        for (case, returned) in [
            ("no status at all", &br#"{"body":"hi"}"#[..]),
            ("a status outside the HTTP range", &br#"{"statusCode":99}"#[..]),
            ("a body that is not a string", &br#"{"statusCode":200,"body":{"a":1}}"#[..]),
            ("a base64 body that is not base64", &br#"{"statusCode":200,"body":"!","isBase64Encoded":true}"#[..]),
            ("headers that are not an object", &br#"{"statusCode":200,"headers":[]}"#[..]),
            ("no JSON object at all", &b"not json"[..]),
        ] {
            let read = proxy_response(returned);
            assert!(
                read.is_err(),
                "case {case:?}: expected no proxy response to be read, got {:?}",
                read.map(|response| response.status)
            );
        }
    }
}
