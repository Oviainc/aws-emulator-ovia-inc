// SPDX-License-Identifier: CC0-1.0

//! The EVENT an HTTP api hands a Lambda function: the proxy event of each payload format
//! version, and the authorizer event a `REQUEST` authorizer's function is called with.
//!
//! "The payload format version specifies the format of the data that API Gateway sends to a
//! Lambda integration, and how API Gateway interprets the response from Lambda"
//! (`Integration$PayloadFormatVersion`, whose values for an HTTP api are `1.0` and `2.0`).
//! The two formats are different documents, not two spellings of one: `2.0` states the
//! request once — a single-valued header map, the cookies lifted out of the `Cookie` header
//! into their own list, the raw path and raw query string — while `1.0` is the format a REST
//! api's proxy integration uses, with both a single-valued and a multi-valued map of
//! headers and of query parameters, and the method and path on the event itself.
//!
//! Everything in an event comes from the REQUEST and from the api's own stored records.
//! Nothing is invented: a member the request carries no value for is left off the event the
//! way API Gateway leaves it off, rather than filled with a placeholder a handler would read
//! as a value.
//!
//! One member is this emulator's rather than AWS's, and is derived rather than fabricated:
//! `requestId` is "the ID of the request" the gateway mints per invoke, so it is minted here
//! per invoke too, unique and stable for the event it is put on.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use ::base64::Engine as _;
use serde_json::{json, Map, Value};

use super::super::data_plane::path_segments;

/// The header a client carries its cookies in (RFC 6265 §5.4), which a `2.0` event lifts
/// out of the header map into its own list: "cookies — an array of the cookies the request
/// carried".
const COOKIE_HEADER: &str = "cookie";

/// How a `2.0` event's `Cookie` header is split into the list: cookie pairs are separated
/// by `; ` (RFC 6265 §4.2.1).
const COOKIE_SEPARATOR: char = ';';

/// How a header or query parameter that appeared more than once is carried in a
/// SINGLE-valued map: one comma-separated value (RFC 9110 §5.3), the form API Gateway
/// documents for a `2.0` event's `headers` and `queryStringParameters`.
const MULTI_VALUE_SEPARATOR: &str = ",";

/// The protocol an event reports the request arrived over. The transport the emulator's
/// front door serves is HTTP/1.1, and the event reports what carried the request rather
/// than what the api could carry.
const REQUEST_PROTOCOL: &str = "HTTP/1.1";

/// The address an event reports as the caller's when the request carries nothing that
/// states it: the loopback address, which is where a client of this emulator reaches it
/// from. `X-Forwarded-For`'s first entry is preferred when a proxy stated it, because that
/// is the address the request came from.
const LOOPBACK_SOURCE_IP: &str = "127.0.0.1";

/// The request header a proxy states the original client's address in (the first entry is
/// the client's).
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The greedy suffix of a path-variable route-key segment (`{proxy+}`), which stands for
/// "the remainder of the path" and so names one parameter holding every remaining segment.
const GREEDY_VARIABLE_SUFFIX: char = '+';

/// The `PayloadFormatVersion` values an HTTP api's Lambda proxy integration admits.
///
/// From `Integration$PayloadFormatVersion`: "Specifies the format of the payload sent to an
/// integration. Required for HTTP APIs. For HTTP APIs, supported values for Lambda proxy
/// integrations are 1.0 and 2.0."
pub const PAYLOAD_FORMAT_1_0: &str = "1.0";
pub const PAYLOAD_FORMAT_2_0: &str = "2.0";

/// What one invoke of a route is, as the event builders read it: the request, and the
/// stored records of the api and stage that resolved it.
///
/// One struct for both event formats and for the authorizer event, so the three documents
/// cannot disagree about what the request was.
#[derive(Debug, Clone)]
pub struct ProxyRequest {
    /// The request's method, exactly as it arrived.
    pub method: String,
    /// The path the api's own endpoint was addressed at, which is what AWS's own event
    /// reports: a NAMED stage's segment included, because that stage's invoke URL carries it
    /// ("the stage name is typically appended to this URI to form a complete path to a
    /// deployed API stage", `Api$ApiEndpoint`), and no stage segment at all for the stage
    /// that serves the api's root.
    pub raw_path: String,
    /// The path within the STAGE, which is what the route key matched.
    pub route_path: String,
    /// The query string as it arrived, with no leading `?`; empty for a request carrying
    /// none.
    pub raw_query_string: String,
    /// Every header, by lowercased name, in the order a `BTreeMap` keeps them; a header
    /// that appeared more than once is one comma-joined value.
    pub headers: BTreeMap<String, String>,
    /// Every query parameter, by name, likewise comma-joined when repeated.
    pub query_parameters: BTreeMap<String, String>,
    /// The request's body as it arrived.
    pub body: Vec<u8>,
    /// The matched route's `RouteKey`.
    pub route_key: String,
    /// The api's own id.
    pub api_id: String,
    /// The stage the request resolved.
    pub stage_name: String,
    /// That stage's variables, which an event carries so a handler can read them.
    pub stage_variables: BTreeMap<String, String>,
    /// The Region the api is reached in.
    pub region: String,
    /// The host the request was addressed to, as the `domainName` the event reports.
    pub domain_name: String,
}

impl ProxyRequest {
    /// The cookies the request carried, as a `2.0` event's own list.
    fn cookies(&self) -> Vec<String> {
        self.headers
            .get(COOKIE_HEADER)
            .map(|value| {
                value
                    .split(COOKIE_SEPARATOR)
                    .map(str::trim)
                    .filter(|pair| !pair.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The header map a `2.0` event carries: every header except the one the cookies were
    /// lifted out of, which that format reports in `cookies` instead.
    fn headers_without_cookies(&self) -> Map<String, Value> {
        self.headers
            .iter()
            .filter(|(name, _)| name.as_str() != COOKIE_HEADER)
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect()
    }

    /// The caller's address, as the event reports it.
    fn source_ip(&self) -> String {
        self.headers
            .get(FORWARDED_FOR_HEADER)
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .unwrap_or(LOOPBACK_SOURCE_IP)
            .to_string()
    }

    /// The `userAgent` the event reports, or `None` for a request naming none.
    fn user_agent(&self) -> Option<String> {
        self.headers.get("user-agent").cloned()
    }

    /// The body as the event carries it, with the flag that says how it is encoded.
    ///
    /// An event is a JSON document, and a JSON string holds TEXT, so a payload that is not
    /// text cannot be carried as itself: "isBase64Encoded — whether the body is
    /// base64-encoded". So a body that is valid UTF-8 travels as the text it is, and one
    /// that is not travels base64-encoded and marked — which is what makes a binary payload
    /// reach the function unchanged, byte for byte, in both directions.
    fn encoded_body(&self) -> (Option<String>, bool) {
        if self.body.is_empty() {
            return (None, false);
        }
        match std::str::from_utf8(&self.body) {
            Ok(text) => (Some(text.to_string()), false),
            Err(_) => (
                Some(::base64::engine::general_purpose::STANDARD.encode(&self.body)),
                true,
            ),
        }
    }

    /// The path parameters the matched route key names, read off the request's own path.
    ///
    /// A `{name}` segment of the key names one parameter holding the request segment at
    /// that position, and the greedy `{name+}` names one holding "the remainder of the
    /// path" — every remaining segment, joined by `/`, which is the value AWS gives
    /// `{proxy+}`.
    pub fn path_parameters(&self) -> BTreeMap<String, String> {
        let mut parameters = BTreeMap::new();
        let requested = path_segments(&self.route_path);
        let Some((_, key_path)) = self.route_key.split_once(' ') else {
            return parameters;
        };
        for (position, expected) in path_segments(key_path).iter().enumerate() {
            let Some(name) = expected
                .strip_prefix('{')
                .and_then(|inner| inner.strip_suffix('}'))
            else {
                continue;
            };
            if let Some(greedy) = name.strip_suffix(GREEDY_VARIABLE_SUFFIX) {
                let remainder = requested
                    .get(position..)
                    .map(|rest| rest.join("/"))
                    .unwrap_or_default();
                parameters.insert(greedy.to_string(), remainder);
                break;
            }
            if let Some(segment) = requested.get(position) {
                parameters.insert(name.to_string(), (*segment).to_string());
            }
        }
        parameters
    }
}

/// What the gateway knows about the invocation itself, beside the request: when it
/// happened, who the api belongs to, and what an authorizer in front of the route decided.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// The account the api belongs to.
    pub account_id: String,
    /// The id this invoke is reported under, minted per invoke ([`mint_request_id`]).
    pub request_id: String,
    /// When the gateway received the request.
    pub received: SystemTime,
    /// The `context` a `REQUEST` authorizer returned with its decision, and the principal
    /// it identified — carried into the integration's event, which is the only way a
    /// handler reads what the authorizer decided.
    pub authorizer_context: Option<Value>,
    /// The principal a policy-document authorizer named, if any.
    pub authorizer_principal: Option<String>,
    /// What the route's JWT authorizer accepted about the caller, for a request it admitted
    /// — carried into the event the same way, and `None` for a route no JWT authorizer
    /// fronts.
    pub admitted_jwt: Option<crate::service::jwt::Accepted>,
}

/// The event a Lambda proxy integration with `PayloadFormatVersion` `2.0` is invoked with.
///
/// Every member is the one AWS documents for the format, and a member with nothing to
/// report is LEFT OFF rather than sent as an empty value: a handler reading
/// `event["cookies"]` on a request that carried none must find nothing there, exactly as it
/// would at AWS.
pub fn event_2_0(request: &ProxyRequest, invocation: &Invocation) -> Value {
    let (body, is_base64_encoded) = request.encoded_body();
    let mut event = Map::new();
    event.insert("version".to_string(), json!(PAYLOAD_FORMAT_2_0));
    event.insert("routeKey".to_string(), json!(request.route_key));
    event.insert("rawPath".to_string(), json!(request.raw_path));
    event.insert(
        "rawQueryString".to_string(),
        json!(request.raw_query_string),
    );
    let cookies = request.cookies();
    if !cookies.is_empty() {
        event.insert("cookies".to_string(), json!(cookies));
    }
    event.insert(
        "headers".to_string(),
        Value::Object(request.headers_without_cookies()),
    );
    if !request.query_parameters.is_empty() {
        event.insert(
            "queryStringParameters".to_string(),
            json!(request.query_parameters),
        );
    }
    event.insert(
        "requestContext".to_string(),
        request_context_2_0(request, invocation),
    );
    let path_parameters = request.path_parameters();
    if !path_parameters.is_empty() {
        event.insert("pathParameters".to_string(), json!(path_parameters));
    }
    if !request.stage_variables.is_empty() {
        event.insert("stageVariables".to_string(), json!(request.stage_variables));
    }
    if let Some(body) = body {
        event.insert("body".to_string(), json!(body));
    }
    event.insert("isBase64Encoded".to_string(), json!(is_base64_encoded));
    Value::Object(event)
}

/// The `requestContext` of a `2.0` event: what the gateway reports about the request it
/// received, with the `http` sub-object holding the method and path that format keeps there.
fn request_context_2_0(request: &ProxyRequest, invocation: &Invocation) -> Value {
    let mut context = Map::new();
    context.insert("accountId".to_string(), json!(invocation.account_id));
    context.insert("apiId".to_string(), json!(request.api_id));
    if let Some(authorizer) = authorizer_2_0(invocation) {
        context.insert("authorizer".to_string(), authorizer);
    }
    context.insert("domainName".to_string(), json!(request.domain_name));
    context.insert(
        "domainPrefix".to_string(),
        json!(domain_prefix(&request.domain_name)),
    );
    let mut http = Map::new();
    http.insert("method".to_string(), json!(request.method));
    http.insert("path".to_string(), json!(request.raw_path));
    http.insert("protocol".to_string(), json!(REQUEST_PROTOCOL));
    http.insert("sourceIp".to_string(), json!(request.source_ip()));
    if let Some(user_agent) = request.user_agent() {
        http.insert("userAgent".to_string(), json!(user_agent));
    }
    context.insert("http".to_string(), Value::Object(http));
    context.insert("requestId".to_string(), json!(invocation.request_id));
    context.insert("routeKey".to_string(), json!(request.route_key));
    context.insert("stage".to_string(), json!(request.stage_name));
    context.insert(
        "time".to_string(),
        json!(common_log_time(invocation.received)),
    );
    context.insert(
        "timeEpoch".to_string(),
        json!(epoch_millis(invocation.received)),
    );
    Value::Object(context)
}

/// The `requestContext.authorizer` of a `2.0` event, for a route an authorizer decided.
///
/// One member per kind of authorizer, so a handler reads the decision of the authorizer its
/// route actually carries: "lambda — the context the Lambda authorizer returned" (the
/// function's OWN object, carried through unchanged, whatever types its members hold), and
/// `jwt` — the `claims` and `scopes` of the token a JWT authorizer accepted, the two members
/// the documented `2.0` event carries for a JWT-authorized route. A route carries one
/// authorizer, so one member is filled; a route with none reports no `authorizer` at all.
fn authorizer_2_0(invocation: &Invocation) -> Option<Value> {
    let mut authorizer = Map::new();
    if let Some(context) = invocation.authorizer_context.clone() {
        authorizer.insert("lambda".to_string(), context);
    }
    if let Some(accepted) = invocation.admitted_jwt.as_ref() {
        authorizer.insert(
            "jwt".to_string(),
            json!({
                "claims": claims_value(accepted),
                "scopes": scopes_value(accepted),
            }),
        );
    }
    (!authorizer.is_empty()).then_some(Value::Object(authorizer))
}

/// The `claims` an accepted token reports: its whole claim set, as the map of TEXT values
/// the event carries, and JSON null for a route whose authorizer accepted no token — the
/// shape the documented event's own `"claims": null` has.
fn claims_value(accepted: &crate::service::jwt::Accepted) -> Value {
    if accepted.claims.is_empty() {
        return Value::Null;
    }
    json!(accepted.claims)
}

/// The `scopes` an accepted token reports: "the scopes parsed from the access token in the
/// incoming request" (`Route$AuthorizationScopes`), and JSON null for a token claiming none
/// — an identity token carries no scope claim, and the documented event reports that absent
/// collection as null rather than as an empty list.
fn scopes_value(accepted: &crate::service::jwt::Accepted) -> Value {
    if accepted.scopes.is_empty() {
        return Value::Null;
    }
    json!(accepted.scopes)
}

/// The event a Lambda proxy integration with `PayloadFormatVersion` `1.0` is invoked with:
/// the format a REST api's proxy integration uses, which an HTTP api also admits.
pub fn event_1_0(request: &ProxyRequest, invocation: &Invocation) -> Value {
    let (body, is_base64_encoded) = request.encoded_body();
    let path_parameters = request.path_parameters();
    let resource = request
        .route_key
        .split_once(' ')
        .map(|(_, path)| path.to_string())
        .unwrap_or_else(|| request.route_path.clone());
    json!({
        "version": PAYLOAD_FORMAT_1_0,
        "resource": resource,
        "path": request.raw_path,
        "httpMethod": request.method,
        "headers": single_valued(&request.headers),
        "multiValueHeaders": multi_valued(&request.headers),
        "queryStringParameters": optional_map(&request.query_parameters),
        "multiValueQueryStringParameters": optional_multi_map(&request.query_parameters),
        "requestContext": request_context_1_0(request, invocation, &resource),
        "pathParameters": optional_map(&path_parameters),
        "stageVariables": optional_map(&request.stage_variables),
        "body": body,
        "isBase64Encoded": is_base64_encoded,
    })
}

/// The `requestContext` of a `1.0` event, which reports the method and path on itself and
/// the caller's address under `identity`.
fn request_context_1_0(
    request: &ProxyRequest,
    invocation: &Invocation,
    resource: &str,
) -> Value {
    let mut context = Map::new();
    context.insert("accountId".to_string(), json!(invocation.account_id));
    context.insert("apiId".to_string(), json!(request.api_id));
    if let Some(authorizer) = authorizer_1_0(invocation) {
        context.insert("authorizer".to_string(), authorizer);
    }
    context.insert("domainName".to_string(), json!(request.domain_name));
    context.insert(
        "domainPrefix".to_string(),
        json!(domain_prefix(&request.domain_name)),
    );
    context.insert(
        "extendedRequestId".to_string(),
        json!(invocation.request_id),
    );
    context.insert("httpMethod".to_string(), json!(request.method));
    context.insert(
        "identity".to_string(),
        json!({
            "sourceIp": request.source_ip(),
            "userAgent": request.user_agent(),
        }),
    );
    context.insert("path".to_string(), json!(request.raw_path));
    context.insert("protocol".to_string(), json!(REQUEST_PROTOCOL));
    context.insert("requestId".to_string(), json!(invocation.request_id));
    context.insert(
        "requestTime".to_string(),
        json!(common_log_time(invocation.received)),
    );
    context.insert(
        "requestTimeEpoch".to_string(),
        json!(epoch_millis(invocation.received)),
    );
    context.insert("resourcePath".to_string(), json!(resource));
    context.insert("stage".to_string(), json!(request.stage_name));
    Value::Object(context)
}

/// The `requestContext.authorizer` of a `1.0` event: the authorizer's context members at
/// the TOP of the object, beside the `principalId` a policy-document authorizer named —
/// the shape a REST proxy event carries, which this format is — and, for a JWT-authorized
/// route, the `claims` and `scopes` of the accepted token, the two members the documented
/// `1.0` event of an HTTP api carries there.
pub(crate) fn authorizer_1_0(invocation: &Invocation) -> Option<Value> {
    let context = invocation.authorizer_context.clone();
    let principal = invocation.authorizer_principal.clone();
    if context.is_none() && principal.is_none() && invocation.admitted_jwt.is_none() {
        return None;
    }
    let mut authorizer = match context {
        Some(Value::Object(members)) => members,
        Some(other) => {
            let mut members = Map::new();
            members.insert("context".to_string(), other);
            members
        }
        None => Map::new(),
    };
    if let Some(principal) = principal {
        authorizer.insert("principalId".to_string(), json!(principal));
    }
    if let Some(accepted) = invocation.admitted_jwt.as_ref() {
        authorizer.insert("claims".to_string(), claims_value(accepted));
        authorizer.insert("scopes".to_string(), scopes_value(accepted));
    }
    Some(Value::Object(authorizer))
}

/// The event a `REQUEST` authorizer's function is called with, in the payload format the
/// authorizer declares.
///
/// `2.0` is the proxy event of that format with the three members an authorizer event adds —
/// `type`, `routeArn` and `identitySource` — and `1.0` is the REST `REQUEST` authorizer
/// event, which names the method ARN it is deciding about instead. Neither carries the
/// request's BODY: an authorizer decides from the request's identity sources and context,
/// and AWS sends no body to it.
pub fn authorizer_event(
    request: &ProxyRequest,
    invocation: &Invocation,
    payload_format: &str,
    identity: &[String],
    route_arn: &str,
) -> Value {
    let bodiless = ProxyRequest {
        body: Vec::new(),
        ..request.clone()
    };
    if payload_format == PAYLOAD_FORMAT_2_0 {
        let mut event = event_2_0(&bodiless, invocation);
        if let Value::Object(members) = &mut event {
            members.insert("type".to_string(), json!("REQUEST"));
            members.insert("routeArn".to_string(), json!(route_arn));
            members.insert("identitySource".to_string(), json!(identity));
            // An authorizer event reports no body at all, so the flag that describes one
            // is not on it either.
            members.remove("isBase64Encoded");
            members.remove("body");
        }
        return event;
    }
    let mut event = event_1_0(&bodiless, invocation);
    if let Value::Object(members) = &mut event {
        members.insert("type".to_string(), json!("REQUEST"));
        members.insert("methodArn".to_string(), json!(route_arn));
        members.insert("identitySource".to_string(), json!(identity));
        members.remove("isBase64Encoded");
        members.remove("body");
    }
    event
}

/// A single-valued JSON map of `values`, always present (a `1.0` event's `headers`).
pub(crate) fn single_valued(values: &BTreeMap<String, String>) -> Value {
    json!(values)
}

/// The multi-valued form of `values`: each name against the list its comma-joined value
/// splits into, which is how a `1.0` event reports a header that appeared more than once.
pub(crate) fn multi_valued(values: &BTreeMap<String, String>) -> Value {
    let multi: BTreeMap<String, Vec<String>> = values
        .iter()
        .map(|(name, value)| {
            (
                name.clone(),
                value
                    .split(MULTI_VALUE_SEPARATOR)
                    .map(str::trim)
                    .map(str::to_string)
                    .collect(),
            )
        })
        .collect();
    json!(multi)
}

/// `values` as a JSON map, or JSON null for an empty one: a `1.0` event reports an absent
/// collection as `null` rather than as an empty object.
pub(crate) fn optional_map(values: &BTreeMap<String, String>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    json!(values)
}

/// [`optional_map`] in the multi-valued form.
pub(crate) fn optional_multi_map(values: &BTreeMap<String, String>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    multi_valued(values)
}

/// The first label of the host the request was addressed to, which for an `execute-api`
/// endpoint is the api's own id: `domainPrefix` is "the first label of the `domainName`".
fn domain_prefix(domain_name: &str) -> String {
    domain_name
        .split('.')
        .next()
        .unwrap_or(domain_name)
        .to_string()
}

/// `received` in the format an event's `time` / `requestTime` member states it: the Common
/// Log Format instant AWS reports (`12/Mar/2020:19:03:58 +0000`).
///
/// The calendar conversion is the maintained library's
/// (`aws_smithy_types::DateTime`), so month lengths and leap years are not re-derived here;
/// only the field ORDER of the format is applied.
pub(crate) fn common_log_time(received: SystemTime) -> String {
    let rendered = ::aws_smithy_types::DateTime::from(received)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    // `YYYY-MM-DDThh:mm:ssZ` — the one form that renderer writes. Anything else is
    // reported as rendered rather than re-ordered on a guess.
    let Some((date, time)) = rendered.split_once('T') else {
        return rendered;
    };
    let fields: Vec<&str> = date.split('-').collect();
    let [year, month, day] = fields.as_slice() else {
        return rendered;
    };
    let Some(month_name) = month
        .parse::<usize>()
        .ok()
        .and_then(|month| MONTH_NAMES.get(month.wrapping_sub(1)))
    else {
        return rendered;
    };
    let seconds = time
        .trim_end_matches('Z')
        .split('.')
        .next()
        .unwrap_or_default();
    format!("{day}/{month_name}/{year}:{seconds} {UTC_OFFSET}")
}

/// The zone a Common Log Format instant states: the gateway reports it in UTC.
const UTC_OFFSET: &str = "+0000";

/// The month abbreviations the Common Log Format names a month with.
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `received` as whole milliseconds since the epoch, which `timeEpoch` /
/// `requestTimeEpoch` state it in. An instant before the epoch reads as the epoch, so the
/// value is always the millisecond count of an instant.
pub(crate) fn epoch_millis(received: SystemTime) -> i64 {
    received
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// A fresh id for one invoke, in the shape `requestId` carries: lowercase hex, long enough
/// that two invokes of the same api never share one.
///
/// It is a value the GATEWAY mints per request rather than anything derived from the
/// request, so it is minted from the clock and from a count that no two invokes of this
/// process share.
pub fn mint_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let mixed = (nanos as u64) ^ (count.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    format!("{mixed:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request against `GET /orders/{id}` of a named stage, carrying two cookies, a
    /// repeated header and a query string.
    fn request() -> ProxyRequest {
        ProxyRequest {
            method: "GET".to_string(),
            raw_path: "/prod/orders/7".to_string(),
            route_path: "/orders/7".to_string(),
            raw_query_string: "a=1&a=2&b=3".to_string(),
            headers: BTreeMap::from([
                ("cookie".to_string(), "one=1; two=2".to_string()),
                ("accept".to_string(), "text/plain,text/html".to_string()),
                ("user-agent".to_string(), "curl".to_string()),
            ]),
            query_parameters: BTreeMap::from([
                ("a".to_string(), "1,2".to_string()),
                ("b".to_string(), "3".to_string()),
            ]),
            body: b"hello".to_vec(),
            route_key: "GET /orders/{id}".to_string(),
            api_id: "abc123".to_string(),
            stage_name: "prod".to_string(),
            stage_variables: BTreeMap::from([("backend".to_string(), "blue".to_string())]),
            region: "us-east-1".to_string(),
            domain_name: "abc123.execute-api.us-east-1.amazonaws.com".to_string(),
        }
    }

    fn invocation() -> Invocation {
        Invocation {
            account_id: "000000000000".to_string(),
            request_id: "0123456789abcdef".to_string(),
            received: UNIX_EPOCH + std::time::Duration::from_millis(1_583_348_638_390),
            authorizer_context: None,
            authorizer_principal: None,
            admitted_jwt: None,
        }
    }

    #[test]
    fn a_2_0_event_reports_the_request_the_way_aws_documents_it() {
        let event = event_2_0(&request(), &invocation());
        // Each row is one member of the documented format and the value this request owes
        // it.
        let cases: [(&str, &str, Value); 8] = [
            ("the format version", "/version", json!("2.0")),
            ("the matched key", "/routeKey", json!("GET /orders/{id}")),
            ("the path including the stage", "/rawPath", json!("/prod/orders/7")),
            ("the query string as it arrived", "/rawQueryString", json!("a=1&a=2&b=3")),
            ("the cookies lifted out of the header", "/cookies", json!(["one=1", "two=2"])),
            ("the path parameters the key names", "/pathParameters", json!({"id": "7"})),
            ("the stage's variables", "/stageVariables", json!({"backend": "blue"})),
            ("a text body, carried as text", "/isBase64Encoded", json!(false)),
        ];
        for (case, pointer, expected) in cases {
            let actual = event.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {pointer} to be {expected}, got {actual}"
            );
        }
        assert_eq!(
            event.pointer("/headers/cookie"),
            None,
            "case {:?}: the cookie header is reported in `cookies`, not in `headers`",
            "a 2.0 event's header map"
        );
        assert_eq!(
            event.pointer("/requestContext/http/method"),
            Some(&json!("GET")),
            "case {:?}: the method belongs to requestContext.http",
            "a 2.0 event's request context"
        );
        assert_eq!(
            event.pointer("/requestContext/timeEpoch"),
            Some(&json!(1_583_348_638_390_i64)),
            "case {:?}: the epoch time is stated in milliseconds",
            "a 2.0 event's request context"
        );
        assert_eq!(
            event.pointer("/requestContext/time"),
            Some(&json!("04/Mar/2020:19:03:58 +0000")),
            "case {:?}: the time is the Common Log Format instant",
            "a 2.0 event's request context"
        );
    }

    #[test]
    fn a_1_0_event_reports_both_the_single_and_the_multi_valued_maps() {
        let event = event_1_0(&request(), &invocation());
        let cases: [(&str, &str, Value); 6] = [
            ("the format version", "/version", json!("1.0")),
            ("the method on the event itself", "/httpMethod", json!("GET")),
            ("the key's path as the resource", "/resource", json!("/orders/{id}")),
            (
                "the single-valued header map",
                "/headers/accept",
                json!("text/plain,text/html"),
            ),
            (
                "the multi-valued header map",
                "/multiValueHeaders/accept",
                json!(["text/plain", "text/html"]),
            ),
            (
                "the multi-valued query map",
                "/multiValueQueryStringParameters/a",
                json!(["1", "2"]),
            ),
        ];
        for (case, pointer, expected) in cases {
            let actual = event.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {pointer} to be {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn a_body_that_is_not_text_travels_base64_encoded_and_marked() {
        // Rows differ along what the payload IS: nothing, text, and bytes no text can hold.
        let cases: [(&str, Vec<u8>, Option<&str>, bool); 3] = [
            ("no body at all", Vec::new(), None, false),
            ("a text body", b"hello".to_vec(), Some("hello"), false),
            (
                "a body that is not valid UTF-8",
                vec![0x89, 0x50, 0x4e, 0x47, 0xff],
                None,
                true,
            ),
        ];
        for (case, body, expected_body, expected_flag) in cases {
            let event = event_2_0(
                &ProxyRequest {
                    body: body.clone(),
                    ..request()
                },
                &invocation(),
            );
            let flag = event.pointer("/isBase64Encoded").cloned();
            assert_eq!(
                flag,
                Some(json!(expected_flag)),
                "case {case:?}: expected isBase64Encoded {expected_flag}, got {flag:?}"
            );
            let carried = event
                .pointer("/body")
                .and_then(Value::as_str)
                .map(str::to_string);
            if expected_flag {
                let decoded = ::base64::engine::general_purpose::STANDARD
                    .decode(carried.clone().unwrap_or_default())
                    .unwrap_or_default();
                assert_eq!(
                    decoded, body,
                    "case {case:?}: the encoded body must decode to the bytes that arrived, \
                     got {carried:?}"
                );
            } else {
                assert_eq!(
                    carried.as_deref(),
                    expected_body,
                    "case {case:?}: expected {expected_body:?}, got {carried:?}"
                );
            }
        }
    }

    #[test]
    fn a_route_keys_variables_name_the_path_parameters_of_the_request() {
        // Rows differ along the KIND of variable the key holds: none, one, two, and the
        // greedy remainder, which holds every segment left.
        let cases: [(&str, &str, &str, Value); 5] = [
            ("a key naming no variable", "GET /orders", "/orders", json!({})),
            ("one variable", "GET /orders/{id}", "/orders/7", json!({"id": "7"})),
            (
                "two variables",
                "GET /orders/{id}/items/{item}",
                "/orders/7/items/3",
                json!({"id": "7", "item": "3"}),
            ),
            (
                "the greedy remainder over several segments",
                "ANY /{proxy+}",
                "/a/b/c",
                json!({"proxy": "a/b/c"}),
            ),
            (
                "the greedy remainder under a literal prefix",
                "GET /files/{path+}",
                "/files/a/b",
                json!({"path": "a/b"}),
            ),
        ];
        for (case, route_key, route_path, expected) in cases {
            let actual = json!(ProxyRequest {
                route_key: route_key.to_string(),
                route_path: route_path.to_string(),
                ..request()
            }
            .path_parameters());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn an_authorizer_event_carries_the_identity_and_the_route_it_decides_about_and_no_body() {
        let arn = "arn:aws:execute-api:us-east-1:000000000000:abc123/prod/GET/orders/7";
        for (case, format, type_pointer, arn_pointer) in [
            ("the 2.0 authorizer event", PAYLOAD_FORMAT_2_0, "/type", "/routeArn"),
            ("the 1.0 authorizer event", PAYLOAD_FORMAT_1_0, "/type", "/methodArn"),
        ] {
            let event = authorizer_event(
                &request(),
                &invocation(),
                format,
                &["token-value".to_string()],
                arn,
            );
            assert_eq!(
                event.pointer(type_pointer),
                Some(&json!("REQUEST")),
                "case {case:?}: expected the event to name its type"
            );
            assert_eq!(
                event.pointer(arn_pointer),
                Some(&json!(arn)),
                "case {case:?}: expected the event to name the route it decides about"
            );
            assert_eq!(
                event.pointer("/identitySource"),
                Some(&json!(["token-value"])),
                "case {case:?}: expected the identity sources the decision is keyed on"
            );
            assert_eq!(
                event.pointer("/body"),
                None,
                "case {case:?}: an authorizer event carries no request body"
            );
        }
    }

    #[test]
    fn an_authorizers_context_reaches_the_integration_event_unchanged() {
        // The value is the function's own JSON, whatever type it holds: an array stays an
        // array, a number a number.
        let context = json!({"tier": ["gold", "silver"], "age": 42, "name": "ada"});
        let invocation = Invocation {
            authorizer_context: Some(context.clone()),
            authorizer_principal: Some("user-7".to_string()),
            ..invocation()
        };
        let two = event_2_0(&request(), &invocation);
        assert_eq!(
            two.pointer("/requestContext/authorizer/lambda"),
            Some(&context),
            "case {:?}: expected the context under requestContext.authorizer.lambda",
            "a 2.0 integration event"
        );
        let one = event_1_0(&request(), &invocation);
        assert_eq!(
            one.pointer("/requestContext/authorizer/tier"),
            Some(&json!(["gold", "silver"])),
            "case {:?}: expected the context's members at the top of the authorizer object",
            "a 1.0 integration event"
        );
        assert_eq!(
            one.pointer("/requestContext/authorizer/principalId"),
            Some(&json!("user-7")),
            "case {:?}: expected the principal the authorizer named",
            "a 1.0 integration event"
        );
    }

    #[test]
    fn a_minted_request_id_is_fresh_for_every_invoke() {
        let first = mint_request_id();
        let second = mint_request_id();
        assert_ne!(
            first, second,
            "case {:?}: two invokes must not share one request id",
            "two invokes of the same process"
        );
        assert!(
            first.len() == 16 && first.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "case {:?}: expected 16 hex digits, got {first:?}",
            "a minted request id"
        );
    }
}
