// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! REST+XML server protocol implementation using `aws-smithy-xml`.
//!
//! This module implements [`ServerProtocol`] for the `restXml` Smithy protocol.
//! Routing is based on HTTP method + URI pattern matching; bodies are
//! serialized/deserialized as XML using `aws-smithy-xml`.
//!
//! HTTP binding (`@httpLabel`, `@httpQuery`, `@httpHeader`, `@httpResponseCode`,
//! `@httpPrefixHeaders`, `@httpQueryParams`) is handled by
//! [`HttpBindingDeserializer`] and [`HttpBindingSerializer`] from
//! [`crate::http_binding`]. The protocol consults the input/output schema's
//! attached HTTP traits at runtime — generated code is protocol-agnostic.

use crate::http_binding::{HttpBindingDeserializer, HttpBindingSerializer};
use crate::protocol::{
    insert_error_response_headers, insert_response_headers, ProtocolError, ServerProtocol,
};
use crate::xml::XmlCodec;
use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_types::body::SdkBody;

/// Response header some restXml services return alongside an error, naming the
/// request. A `noErrorWrapping` error echoes it, and the body's `<RequestId>`
/// carries the same value.
const REQUEST_ID_HEADER: &str = "x-amz-request-id";
/// Second request-correlation header those services return. Mirrors the body's
/// `<HostId>`.
const EXTENDED_REQUEST_ID_HEADER: &str = "x-amz-id-2";

/// A fresh request-id for one unwrapped error response, carried identically in
/// the body's `<RequestId>` element and the `x-amz-request-id` header. Each
/// response carries its own id so two responses are distinguishable; the id
/// names a request for correlation and is not a security token, so a random
/// value is correct. Generate it once per response and use the one value in
/// both places.
fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A fresh extended request-id (`<HostId>` / `x-amz-id-2`) for one unwrapped
/// error response. Unlike the request-id it is not a UUID: the S3 `x-amz-id-2`
/// shape is a 64-lowercase-hex-character string, so hex-encode 32 random bytes
/// (two v4 UUIDs' worth). Like the request-id, generate it once per response
/// and use the one value in both the body's `<HostId>` and the header.
fn new_host_id() -> String {
    let mut hex = String::with_capacity(64);
    for byte in uuid::Uuid::new_v4()
        .as_bytes()
        .iter()
        .chain(uuid::Uuid::new_v4().as_bytes().iter())
    {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Build an HTTP status code from a u16, falling back to 400 on invalid values.
fn build_status_code(http_status: u16) -> aws_smithy_runtime_api::http::StatusCode {
    aws_smithy_runtime_api::http::StatusCode::try_from(http_status)
        .unwrap_or(aws_smithy_runtime_api::http::StatusCode::try_from(400u16).expect("400"))
}

/// A request-observable binding that a route's operation marks `@required`.
///
/// REST services overload one method + URI template across several operations
/// and rely on the *shape of the request* to pick between them. The signal is
/// in the model: a member that is both `@required` and bound to a header or a
/// query parameter must be present for that operation to be a legal target.
/// `@httpLabel` members are excluded — they are satisfied by the path match
/// itself, so they carry no discriminating information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredBinding {
    /// A `@required` + `@httpHeader` member, by wire header name.
    Header(&'static str),
    /// A `@required` + `@httpQuery` member, by wire query-parameter name.
    Query(&'static str),
}

/// A single route entry: HTTP method + URI pattern → operation name, plus the
/// operation's `@required` header/query bindings used to break ties between
/// routes that share a method + URI template (see `route_lookup`).
#[derive(Debug, Clone)]
pub struct Route {
    method: &'static str,
    pattern: &'static str,
    operation: &'static str,
    required: &'static [RequiredBinding],
}

impl Route {
    pub const fn new(method: &'static str, pattern: &'static str, operation: &'static str) -> Self {
        Self {
            method,
            pattern,
            operation,
            required: &[],
        }
    }

    /// Attach the operation's `@required` header/query bindings.
    ///
    /// Generated code calls this for every operation that has at least one;
    /// omitting it leaves the route with no required bindings, which matches
    /// the previous behaviour exactly.
    pub const fn with_required(mut self, required: &'static [RequiredBinding]) -> Self {
        self.required = required;
        self
    }
}

/// Compiled segment of a URI pattern.
#[derive(Debug)]
enum Segment<'a> {
    Literal(&'a str),
    Label,
    Greedy,
}

/// Parse a URI pattern like `/2013-04-01/hostedzone/{Id}` into segments.
fn parse_pattern(pattern: &str) -> Vec<Segment<'_>> {
    let trimmed = pattern.trim_matches('/');
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split('/')
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                let inner = &seg[1..seg.len() - 1];
                if inner.ends_with('+') {
                    Segment::Greedy
                } else {
                    Segment::Label
                }
            } else {
                Segment::Literal(seg)
            }
        })
        .collect()
}

/// REST+XML server protocol.
///
/// Constructed with a static route table. Implements `ServerProtocol` for
/// HTTP method + path routing, XML body ser/deser, and XML error envelopes.
#[derive(Debug)]
pub struct RestXmlServerProtocol {
    routes: Vec<Route>,
    namespace: Option<&'static str>,
    no_error_wrapping: bool,
}

impl RestXmlServerProtocol {
    pub fn new(routes: &[Route]) -> Self {
        Self {
            routes: routes.to_vec(),
            namespace: None,
            no_error_wrapping: false,
        }
    }

    pub fn with_namespace(mut self, ns: &'static str) -> Self {
        self.namespace = Some(ns);
        self
    }

    /// Drop the `<ErrorResponse><Error>…</Error></ErrorResponse>` wrapper from
    /// error responses, emitting a bare `<Error>…</Error>` document plus the
    /// `x-amz-request-id` / `x-amz-id-2` response headers.
    ///
    /// Some restXml services declare `noErrorWrapping` on the protocol trait —
    /// their SDKs read the error `<Code>` from an UNWRAPPED `<Error>` root and
    /// find nothing when it is wrapped. The default (`false`) keeps the wrapped
    /// envelope, which is what every other restXml service expects.
    pub fn with_no_error_wrapping(mut self, no_error_wrapping: bool) -> Self {
        self.no_error_wrapping = no_error_wrapping;
        self
    }

    /// Find the matching route for a request, returning the operation name.
    fn find_route(
        &self,
        method: &str,
        path: &str,
        query: &str,
        has_header: &dyn Fn(&str) -> bool,
    ) -> Option<&'static str> {
        route_lookup(&self.routes, method, path, query, has_header)
    }

    /// Serialize an error in the unwrapped shape (`noErrorWrapping`): a bare
    /// `<Error>` root with NO namespace, no `<Type>` element, trailing
    /// `<RequestId>`/`<HostId>` elements, and matching `x-amz-request-id` /
    /// `x-amz-id-2` response headers.
    ///
    /// The unwrapped `<Error>` bare root carries NO `xmlns` attribute — the
    /// service namespace applies only to the wrapped `<ErrorResponse xmlns="...">`
    /// envelope, not to the unwrapped bare root. SDK error parsers read `Code`
    /// from a namespace-free `<Error>` and find nothing when `xmlns` is present.
    fn serialize_error_unwrapped(
        &self,
        error: &dyn SerializableStruct,
        error_code: &str,
        http_status: u16,
    ) -> Result<Response, ProtocolError> {
        // Codec with no namespace — the unwrapped bare root is namespace-free.
        let codec = XmlCodec::new();
        let mut ser = codec.create_serializer();

        // The unwrapped shape carries one request-id and one host-id, each used
        // in both the body element and the matching response header below.
        let request_id = new_request_id();
        let host_id = new_host_id();

        static ERROR_ELEMENT_SCHEMA: Schema = Schema::new_struct(
            ShapeId::from_static("smithy.framework#Error", "smithy.framework", "Error"),
            ShapeType::Structure,
            &[],
        );

        // Bare `<Error>` root — no `<ErrorResponse>` wrapper, no `<Type>`.
        ser.open_element(&ERROR_ELEMENT_SCHEMA)?;
        ser.write_text_element("Code", error_code)?;

        // Serialize through HttpBindingSerializer so @httpHeader members
        // go to headers rather than the XML body.
        let mut binder = HttpBindingSerializer::new(ser);
        error.serialize_members(&mut binder)?;
        let (collected, mut ser) = binder.finish();

        // Close with the request-correlation elements whose values the two
        // response headers below mirror.
        ser.write_text_element("RequestId", &request_id)?;
        ser.write_text_element("HostId", &host_id)?;
        ser.close_element(&ERROR_ELEMENT_SCHEMA)?;

        let body_bytes = ser.finish();

        let status = build_status_code(http_status);
        let mut resp = Response::new(status, SdkBody::from(body_bytes));
        resp.headers_mut().insert("content-type", "application/xml");
        resp.headers_mut().insert(REQUEST_ID_HEADER, request_id);
        resp.headers_mut()
            .insert(EXTENDED_REQUEST_ID_HEADER, host_id);
        // Request-influenced `@httpHeader` values go through the fail-loud
        // helper, which omits any the HTTP layer cannot represent rather than
        // downgrading a modeled error to a transport failure. The server-chosen
        // request-id/host-id above are always representable and stay on `insert`.
        insert_error_response_headers(&mut resp, collected.headers);
        Ok(resp)
    }

    /// Serialize an error in the wrapped shape (the Smithy restXml default):
    /// `<ErrorResponse xmlns=".."><Error><Type>..</Type><Code>..</Code>...</Error></ErrorResponse>`.
    ///
    /// A service whose model carries no `@xmlNamespace` gets no `xmlns`
    /// attribute — Smithy leaves it off, and stamping some other service's
    /// namespace on the envelope is worse than omitting it. `XmlCodec` already
    /// treats `None` as "no attribute", so the error path must not demand one.
    fn serialize_error_wrapped(
        &self,
        error: &dyn SerializableStruct,
        error_code: &str,
        http_status: u16,
    ) -> Result<Response, ProtocolError> {
        // Codec with the service namespace, if any.
        let codec = match self.namespace {
            Some(ns) => XmlCodec::new().with_namespace(ns),
            None => XmlCodec::new(),
        };
        let mut ser = codec.create_serializer();

        let error_type = if http_status >= 500 {
            "Receiver"
        } else {
            "Sender"
        };

        static ERROR_RESPONSE_SCHEMA: Schema = Schema::new_struct(
            ShapeId::from_static(
                "smithy.framework#ErrorResponse",
                "smithy.framework",
                "ErrorResponse",
            ),
            ShapeType::Structure,
            &[],
        );
        static ERROR_ELEMENT_SCHEMA: Schema = Schema::new_struct(
            ShapeId::from_static("smithy.framework#Error", "smithy.framework", "Error"),
            ShapeType::Structure,
            &[],
        );

        // `<ErrorResponse xmlns="..."><Error><Type>..</Type><Code>..</Code>...</Error></ErrorResponse>`
        ser.open_element(&ERROR_RESPONSE_SCHEMA)?;
        ser.open_element(&ERROR_ELEMENT_SCHEMA)?;
        ser.write_text_element("Type", error_type)?;
        ser.write_text_element("Code", error_code)?;

        // Serialize through HttpBindingSerializer so @httpHeader members
        // go to headers rather than the XML body.
        let mut binder = HttpBindingSerializer::new(ser);
        error.serialize_members(&mut binder)?;
        let (collected, mut ser) = binder.finish();

        ser.close_element(&ERROR_ELEMENT_SCHEMA)?;
        ser.close_element(&ERROR_RESPONSE_SCHEMA)?;

        let body_bytes = ser.finish();

        let status = build_status_code(http_status);
        let mut resp = Response::new(status, SdkBody::from(body_bytes));
        resp.headers_mut().insert("content-type", "application/xml");
        // Request-influenced `@httpHeader` values go through the fail-loud
        // helper, which omits any the HTTP layer cannot represent rather than
        // downgrading a modeled error to a transport failure.
        insert_error_response_headers(&mut resp, collected.headers);
        Ok(resp)
    }
}

/// A `has_header` probe for a route lookup that has no request headers to
/// consult. Every [`RequiredBinding::Header`] then reads as absent, so an
/// operation that requires a header is never selected.
#[cfg(test)]
pub(crate) fn no_headers(_name: &str) -> bool {
    false
}

/// Match an HTTP method + request path (and query string) against a route
/// table, returning the operation name of the most specific match.
///
/// Specificity is a two-level key: **path literals first, then matched query
/// literals**. AWS restXml (S3-class) services frequently overload the same
/// method + URI template and disambiguate operations purely by a static query
/// literal baked into the `@http.uri` pattern — e.g. on `GET /{Bucket}`:
///
/// * `ListObjects`      → `/{Bucket}`            (bare template, no query)
/// * `ListObjectsV2`    → `/{Bucket}?list-type=2`
/// * `GetBucketLocation`→ `/{Bucket}?location`
///
/// A route that pins a query key/value only matches when the request carries
/// it, and — on an otherwise-equal path — wins over the bare template. Routes
/// with no query literal keep routing exactly as before (no regression).
///
/// ## Required-binding disambiguation
///
/// Query literals alone cannot separate S3's object-write operations, because
/// AWS SDKs do not put the model's `x-id` discriminator on the wire (botocore's
/// S3 model does not even carry it), so `x-id` is only ever a *bonus* signal
/// (see [`query_match_score`]). Four operations then collapse onto one route:
///
/// ```text
/// PutObject / CopyObject / UploadPart / UploadPartCopy  ->  PUT /{Bucket}/{Key+}
/// DeleteObject / AbortMultipartUpload                   ->  DELETE /{Bucket}/{Key+}
/// ```
///
/// Real S3 tells them apart by the *shape of the request*, and the model says
/// how: `CopyObject` marks `CopySource` `@required` + `@httpHeader
/// x-amz-copy-source`, `UploadPart` marks `PartNumber`/`UploadId` `@required` +
/// `@httpQuery`, and `PutObject` requires neither. So a candidate route is
/// **rejected** when any of its [`RequiredBinding`]s is absent from the request,
/// and among the survivors the one satisfying the *most* required bindings wins.
/// A plain `PUT` of an object body therefore reaches `PutObject`, adding
/// `x-amz-copy-source` reaches `CopyObject`, and adding `partNumber`+`uploadId`
/// reaches `UploadPart` — with no S3-specific knowledge in the router.
///
/// Precedence is the triple **(path literals, matched query literals, satisfied
/// required bindings)**, so an explicit `x-id` (which newer SDKs *do* send)
/// still outranks a request-shape guess.
///
/// Shared by [`RestXmlServerProtocol`] and
/// [`crate::rest_json::RestJsonServerProtocol`] — both REST protocols route
/// identically off the URI pattern table; only the body codec differs.
pub(crate) fn route_lookup(
    routes: &[Route],
    method: &str,
    path: &str,
    query: &str,
    has_header: &dyn Fn(&str) -> bool,
) -> Option<&'static str> {
    let path_trimmed = path.trim_matches('/');
    let path_segments: Vec<&str> = if path_trimmed.is_empty() {
        Vec::new()
    } else {
        path_trimmed.split('/').collect()
    };

    // Best match, keyed by (path literal count, matched query-literal count,
    // satisfied required-binding count).
    let mut best: Option<(&'static str, (usize, usize, usize))> = None;

    for route in routes {
        if route.method != method {
            continue;
        }

        // Split the route pattern into its path portion and any static query
        // literals (`/{Bucket}?list-type=2` → path `/{Bucket}`, query
        // `list-type=2`). Only the path portion drives segment matching.
        let (path_pattern, query_pattern) = split_pattern_query(route.pattern);
        let pattern_segments = parse_pattern(path_pattern);
        let pat_len = pattern_segments.len();

        // Check segment count (greedy matches any length >= pattern segments - 1)
        let has_greedy = pattern_segments
            .iter()
            .any(|s| matches!(s, Segment::Greedy));
        if !has_greedy && pat_len != path_segments.len() {
            continue;
        }
        if has_greedy && path_segments.len() < pat_len {
            continue;
        }

        // Try to match
        let mut matched = true;
        let mut literal_count = 0;
        let mut seg_idx = 0;
        for seg in &pattern_segments {
            match seg {
                Segment::Literal(lit) => {
                    if seg_idx >= path_segments.len() || path_segments[seg_idx] != *lit {
                        matched = false;
                        break;
                    }
                    literal_count += 1;
                    seg_idx += 1;
                }
                Segment::Label => {
                    if seg_idx >= path_segments.len() {
                        matched = false;
                        break;
                    }
                    seg_idx += 1;
                }
                Segment::Greedy => {
                    // Consumes rest
                    seg_idx = path_segments.len();
                }
            }
        }
        if !matched {
            continue;
        }
        if seg_idx != path_segments.len() {
            continue;
        }

        // Query-literal disambiguation. A route that requires a query literal
        // only matches when the request carries it; a route with none always
        // passes. `query_score` counts the matched literals so a query-pinned
        // operation outranks the bare template on the same path.
        let query_score = match query_match_score(query_pattern, query) {
            Some(n) => n,
            None => continue,
        };

        // Required-binding disambiguation: every `@required` header/query
        // binding the operation declares must actually be on the request, or
        // this operation is not a legal target for it.
        let required_score =
            match required_binding_score(route.required, &parse_query_pairs(query), has_header) {
                Some(n) => n,
                None => continue,
            };

        // Precedence: more path literals first, then more matched query
        // literals, then more satisfied required bindings.
        let key = (literal_count, query_score, required_score);
        match best {
            None => best = Some((route.operation, key)),
            Some((_, best_key)) if key > best_key => best = Some((route.operation, key)),
            _ => {}
        }
    }

    best.map(|(op, _)| op)
}

/// Split a route/URI pattern into its path portion and the raw query-literal
/// portion (everything after the first `?`), if any.
fn split_pattern_query(pattern: &str) -> (&str, Option<&str>) {
    match pattern.find('?') {
        Some(i) => (&pattern[..i], Some(&pattern[i + 1..])),
        None => (pattern, None),
    }
}

/// Parse a raw query string (`a=1&b&c=2`) into a list of `(key, Option<value>)`
/// pairs. A bare key (no `=`) yields `None`; an empty segment is skipped.
/// Comparisons are literal — AWS query-literal discriminators (`list-type=2`,
/// `location`, `uploads`, `versions`, `acl`, …) are plain ASCII.
fn parse_query_pairs(raw: &str) -> Vec<(&str, Option<&str>)> {
    raw.split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| match kv.find('=') {
            Some(i) => (&kv[..i], Some(&kv[i + 1..])),
            None => (kv, None),
        })
        .collect()
}

/// Score how well a route's static query literals match the request query.
///
/// Returns `Some(n)` — the number of satisfied literals — when **every**
/// required literal is present, or `None` when any is missing (the route must
/// not match). A `key=value` literal requires that exact key/value; a bare
/// `key` literal (`?location`) requires the key with any value (including
/// empty). A route with no query literals always matches with `Some(0)`.
fn query_match_score(pattern_query: Option<&str>, request_query: &str) -> Option<usize> {
    let required = match pattern_query {
        None => return Some(0),
        Some(q) => parse_query_pairs(q),
    };
    if required.is_empty() {
        return Some(0);
    }
    let actual = parse_query_pairs(request_query);
    let mut matched = 0usize;
    for &(rk, rv) in &required {
        let found = actual.iter().any(|&(ak, av)| {
            ak == rk
                && match rv {
                    Some(expected) => av == Some(expected),
                    None => true,
                }
        });
        if !found {
            // `x-id` is a model-level disambiguation annotation that AWS
            // SDKs do not reliably put on the wire (botocore's S3 model
            // does not carry it at all), so it can only ever *add* signal:
            // when present it boosts the score, when absent the route still
            // matches. Requiring it would make PutObject, DeleteObject and
            // UploadPart unreachable from boto3. Ties between the routes
            // that differ only by `x-id` are broken by
            // [`required_binding_score`], not by iteration order.
            if rk == "x-id" {
                continue;
            }
            return None;
        }
        matched += 1;
    }
    Some(matched)
}

/// Score how many of a route's `@required` header/query bindings the request
/// satisfies.
///
/// Returns `Some(n)` when **every** required binding is present (`n` being
/// how many there were), or `None` when any is missing — in which case the
/// route must not match at all, because the operation could not be invoked
/// without it. A route with no required bindings always scores `Some(0)`.
fn required_binding_score(
    required: &[RequiredBinding],
    request_query: &[(&str, Option<&str>)],
    has_header: &dyn Fn(&str) -> bool,
) -> Option<usize> {
    for binding in required {
        let present = match binding {
            RequiredBinding::Header(name) => has_header(name),
            RequiredBinding::Query(name) => request_query.iter().any(|&(k, _)| k == *name),
        };
        if !present {
            return None;
        }
    }
    Some(required.len())
}

/// Whether a request body carries no XML document at all.
///
/// A body that is empty, or that holds nothing but XML-insignificant
/// whitespace, has no root element. Such a body must never reach the XML
/// parser: S3 `CreateBucket` sends an empty body in `us-east-1`, and some SDKs
/// pad an otherwise-omitted body with a newline. Note that an empty slice is
/// blank, since `all` is vacuously true.
///
/// Only ever ask this about a body the XML parser would actually read — i.e. a
/// structured `@httpPayload` target or the payload-free path. Whitespace is
/// XML-insignificant, but it is NOT insignificant to a scalar (`blob`/`string`)
/// `@httpPayload` member, which binds the body verbatim; see
/// [`is_structured_payload`].
fn is_blank_xml_body(body_bytes: &[u8]) -> bool {
    body_bytes.iter().all(|b| b.is_ascii_whitespace())
}

/// Whether a `@httpPayload` member's body is a whole XML document rather than
/// raw bytes.
///
/// A structured target (`structure`/`union`/`list`/`map`/`document`) means the
/// body IS that shape encoded as XML, so the body codec parses it. Every other
/// target is a scalar (`blob`/`string`) bound to the raw body verbatim, with no
/// codec involved. This classification MUST stay identical to the one in
/// `HttpBindingDeserializer::read_struct`, which decides which of the two
/// deserializers actually consumes the body.
fn is_structured_payload(member: &Schema) -> bool {
    matches!(
        member.shape_type(),
        ShapeType::Structure
            | ShapeType::Union
            | ShapeType::List
            | ShapeType::Map
            | ShapeType::Document
    )
}

/// Extract the path component of a request URI, stripping any scheme/authority
/// prefix and query string.
pub(crate) fn request_path(uri: &str) -> &str {
    if uri.starts_with('/') {
        uri.split('?').next().unwrap_or(uri)
    } else {
        uri.find("://")
            .and_then(|scheme_end| uri[scheme_end + 3..].find('/'))
            .map(|path_start| {
                let abs_start = uri.find("://").unwrap() + 3 + path_start;
                let path_str = &uri[abs_start..];
                path_str.split('?').next().unwrap_or(path_str)
            })
            .unwrap_or("/")
    }
}

/// Extract the query string component (everything after `?`) from a URI.
pub(crate) fn request_query(uri: &str) -> &str {
    match uri.find('?') {
        Some(i) => &uri[i + 1..],
        None => "",
    }
}

/// Collect a request's headers into a `(name, value)` Vec preserving original
/// case so `@httpPrefixHeaders` can reconstruct suffixes.
pub(crate) fn collect_headers(request: &Request) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let headers = request.headers();
    let names: std::collections::BTreeSet<&str> = headers.iter().map(|(n, _)| n).collect();
    for name in names {
        for value in headers.get_all(name) {
            out.push((name.to_string(), value.to_string()));
        }
    }
    out
}

impl ServerProtocol for RestXmlServerProtocol {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        static ID: ShapeId =
            ShapeId::from_static("aws.protocols#restXml", "aws.protocols", "restXml");
        &ID
    }

    fn routing_key<'a>(&self, request: &'a Request) -> Result<&'a str, ProtocolError> {
        let uri = request.uri();
        let path = request_path(uri);
        let query = request_query(uri);
        let method = request.method();

        self.find_route(method, path, query, &|name| {
            request.headers().get(name).is_some()
        })
        .ok_or_else(|| ProtocolError::UnknownOperation(format!("{} {}", method, path)))
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        let body_bytes = request
            .body()
            .bytes()
            .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;

        let path = request_path(request.uri()).to_string();
        let query = request_query(request.uri()).to_string();
        let headers = collect_headers(request);
        let uri_pattern = input_schema.http().map(|h| h.uri().to_string());

        // `@httpPayload`: the body IS one member's value, so the XML codec must
        // not sweep it as a member element. Wire BOTH sources so
        // `HttpBindingDeserializer::read_struct` can pick per the payload
        // member's target shape: the raw bytes for a scalar (`blob`/`string`)
        // payload, and the real XML body codec for a structured
        // (`structure`/`union`/`list`/`map`/`document`) payload (parsed as the
        // whole body). Mirrors the restJson1 payload branch in
        // `rest_json::deserialize_input`.
        let payload_member = input_schema
            .members()
            .iter()
            .find(|m| m.http_payload().is_some());
        if let Some(payload_member) = payload_member {
            // When the body carries no document and the payload member exists
            // but is optional (e.g. S3 CreateBucket's
            // `CreateBucketConfiguration` in us-east-1), treat it as absent
            // rather than feeding blank bytes into the XML parser (which would
            // reject "empty XML document"). The payload member will
            // deserialize as `None`.
            //
            // This short-circuit is for STRUCTURED payloads only. A scalar
            // (`blob`/`string`) payload binds the body verbatim, so a
            // whitespace-only body is that member's LITERAL VALUE — `PUT /obj`
            // with a body of `"   "` stores three spaces, and dropping them
            // would silently corrupt the object. Falling through is safe even
            // for a blank scalar body: `create_deserializer` only wraps the
            // bytes, and `HttpBindingDeserializer::read_struct` routes a scalar
            // payload to `PayloadValueDeserializer`, so the XML parser never
            // sees them.
            if is_structured_payload(payload_member) && is_blank_xml_body(body_bytes) {
                let body = EmptyDeserializer;
                let d: HttpBindingDeserializer<EmptyDeserializer> = HttpBindingDeserializer::new(
                    uri_pattern.as_deref(),
                    &path,
                    &query,
                    headers,
                    body,
                );
                return Ok(Box::new(d));
            }
            let codec = XmlCodec::new();
            let body = codec.create_deserializer(body_bytes);
            let d =
                HttpBindingDeserializer::new(uri_pattern.as_deref(), &path, &query, headers, body)
                    .with_payload_bytes(body_bytes.to_vec());
            return Ok(Box::new(d));
        }

        // Empty body: still need an HttpBindingDeserializer so HTTP-bound
        // members are read; but no body. Wrap an empty body deserializer.
        if is_blank_xml_body(body_bytes) {
            let body = EmptyDeserializer;
            let d: HttpBindingDeserializer<EmptyDeserializer> =
                HttpBindingDeserializer::new(uri_pattern.as_deref(), &path, &query, headers, body);
            return Ok(Box::new(d));
        }

        let codec = XmlCodec::new();
        let body = codec.create_deserializer(body_bytes);
        let d = HttpBindingDeserializer::new(uri_pattern.as_deref(), &path, &query, headers, body);
        Ok(Box::new(d))
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        _operation_name: &str,
        success_status: u16,
    ) -> Result<Response, ProtocolError> {
        let codec = if let Some(ns) = self.namespace {
            XmlCodec::new().with_namespace(ns)
        } else {
            XmlCodec::new()
        };

        // Classify the output body shape up front:
        //  * a `smithy.api#Unit` output has NO body — emit an empty body, never
        //    a bogus `<Unit/>` element (and honor any `@httpResponseCode`);
        //  * a structured `@httpPayload` member (`structure`/`union`/`list`/
        //    `map`/`document`) IS the whole body, serialized WITHOUT the
        //    `<Output>…</Output>` envelope (its own element is the root);
        //  * everything else uses the normal XML envelope, with a scalar
        //    (`blob`/`string`) `@httpPayload` still diverted to `body_override`.
        let is_unit = crate::http_binding::is_unit_shape(output_schema);
        let structured_payload = output_schema
            .members()
            .iter()
            .any(|m| m.http_payload().is_some() && is_structured_payload(m));

        let body = codec.create_serializer();
        let (collected, xml_bytes) = if is_unit {
            // No body: still run members so any @httpHeader / @httpResponseCode
            // bindings are captured, but emit nothing.
            let mut binder = HttpBindingSerializer::new(body);
            output.serialize_members(&mut binder)?;
            let (collected, _body) = binder.finish();
            (collected, Vec::new())
        } else if structured_payload {
            // The structured payload member IS the whole body: serialize the
            // members straight into the body serializer (the payload member
            // writes its own root element; all other members are HTTP-bound and
            // captured as headers / status), with no <Output> envelope.
            let mut binder = HttpBindingSerializer::new(body);
            output.serialize_members(&mut binder)?;
            let (collected, body) = binder.finish();
            (collected, body.finish())
        } else {
            // Normal path: open the XML envelope, serialize members through the
            // binder (which intercepts @httpHeader/@httpResponseCode), close.
            let mut body = body;
            body.open_element(output_schema)?;
            let mut binder = HttpBindingSerializer::new(body);
            output.serialize_members(&mut binder)?;
            let (collected, mut body) = binder.finish();
            body.close_element(output_schema)?;
            (collected, body.finish())
        };

        // Status code: a `@httpResponseCode` member wins; otherwise the
        // operation's success status.
        let status = collected.status_code.unwrap_or(success_status);

        // `@httpPayload`: a scalar (blob/string) member is diverted by the
        // `HttpBindingSerializer` into `body_override` (with the content-type
        // its target type maps to: blob → application/octet-stream, string →
        // text/plain). A structured payload has no override — its serialized
        // bytes above ARE the whole body (application/xml). Mirrors the
        // restJson1 payload branch in `rest_json::serialize_output`.
        let (body_bytes, content_type) = match collected.body_override {
            Some((raw, ct)) => (raw, ct),
            None => (xml_bytes, "application/xml"),
        };

        let mut resp = Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).unwrap_or(
                aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            ),
            SdkBody::from(body_bytes),
        );
        resp.headers_mut().insert("content-type", content_type);
        insert_response_headers(&mut resp, collected.headers)?;
        Ok(resp)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        #[allow(unused)] error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Response, ProtocolError> {
        // Note: error_schema is the top-level shape schema (e.g. InvalidInput).
        // Unlike serialize_output (which uses output_schema for the root element
        // name), restXml errors always use a fixed <ErrorResponse><Error> envelope.
        // The member schemas (which drive HTTP binding and XML element names) are
        // embedded in the generated SerializableStruct impl, not read from here.
        // The http_status is passed explicitly rather than derived from the schema.
        //
        // Two envelope shapes, selected by the model's `noErrorWrapping`:
        //
        //  * default (wrapped) — the Smithy restXml spec default:
        //    `<ErrorResponse xmlns=".."><Error><Type>..</Type><Code>..</Code>..</Error></ErrorResponse>`.
        //  * unwrapped (`noErrorWrapping`) — a bare `<Error>` root with no
        //    `<Type>` and trailing `<RequestId>`/`<HostId>` elements, plus the
        //    `x-amz-request-id` / `x-amz-id-2` response headers. An SDK of a
        //    service that sets this reads the error `<Code>` from the `<Error>`
        //    root and finds nothing when the wrapper is present.
        if self.no_error_wrapping {
            self.serialize_error_unwrapped(error, error_code, http_status)
        } else {
            self.serialize_error_wrapped(error, error_code, http_status)
        }
    }
}

/// A no-op deserializer for requests with empty bodies.
pub(crate) struct EmptyDeserializer;

impl ShapeDeserializer for EmptyDeserializer {
    fn read_struct(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(
            &Schema,
            &mut dyn ShapeDeserializer,
        ) -> Result<(), aws_smithy_schema::serde::SerdeError>,
    ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
        // No members to read from an empty body
        Ok(())
    }

    fn read_list(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(
            &mut dyn ShapeDeserializer,
        ) -> Result<(), aws_smithy_schema::serde::SerdeError>,
    ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }

    fn read_map(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(
            String,
            &mut dyn ShapeDeserializer,
        ) -> Result<(), aws_smithy_schema::serde::SerdeError>,
    ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }

    fn read_boolean(
        &mut self,
        _schema: &Schema,
    ) -> Result<bool, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_byte(&mut self, _schema: &Schema) -> Result<i8, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_short(
        &mut self,
        _schema: &Schema,
    ) -> Result<i16, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_integer(
        &mut self,
        _schema: &Schema,
    ) -> Result<i32, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_long(&mut self, _schema: &Schema) -> Result<i64, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_float(
        &mut self,
        _schema: &Schema,
    ) -> Result<f32, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_double(
        &mut self,
        _schema: &Schema,
    ) -> Result<f64, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_big_integer(
        &mut self,
        _schema: &Schema,
    ) -> Result<aws_smithy_types::BigInteger, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_big_decimal(
        &mut self,
        _schema: &Schema,
    ) -> Result<aws_smithy_types::BigDecimal, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_string(
        &mut self,
        _schema: &Schema,
    ) -> Result<String, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_blob(
        &mut self,
        _schema: &Schema,
    ) -> Result<aws_smithy_types::Blob, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_timestamp(
        &mut self,
        _schema: &Schema,
    ) -> Result<aws_smithy_types::DateTime, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn read_document(
        &mut self,
        _schema: &Schema,
    ) -> Result<aws_smithy_types::Document, aws_smithy_schema::serde::SerdeError> {
        Err(aws_smithy_schema::serde::SerdeError::custom("empty body"))
    }
    fn is_null(&self) -> bool {
        true
    }
    fn container_size(&self) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_runtime_api::http::Request;
    use aws_smithy_schema::serde::SerdeError;
    use aws_smithy_types::body::SdkBody;

    // S3-style overloaded route table: several ops share `GET /{Bucket}` and are
    // disambiguated only by a static query literal from `@http.uri`. The
    // object-write ops share `PUT`/`DELETE /{Bucket}/{Key+}` and are separated
    // only by their `@required` header/query bindings (see `RequiredBinding`).
    const COPY_SOURCE: &[RequiredBinding] = &[RequiredBinding::Header("x-amz-copy-source")];
    const PART_UPLOAD: &[RequiredBinding] = &[
        RequiredBinding::Query("partNumber"),
        RequiredBinding::Query("uploadId"),
    ];
    const PART_UPLOAD_COPY: &[RequiredBinding] = &[
        RequiredBinding::Header("x-amz-copy-source"),
        RequiredBinding::Query("partNumber"),
        RequiredBinding::Query("uploadId"),
    ];
    const UPLOAD_ID: &[RequiredBinding] = &[RequiredBinding::Query("uploadId")];

    fn s3_routes() -> Vec<Route> {
        vec![
            Route::new("GET", "/{Bucket}", "ListObjects"),
            Route::new("GET", "/{Bucket}?list-type=2", "ListObjectsV2"),
            Route::new("GET", "/{Bucket}?location", "GetBucketLocation"),
            Route::new("GET", "/{Bucket}?uploads", "ListMultipartUploads"),
            Route::new("PUT", "/{Bucket}", "CreateBucket"),
            Route::new("GET", "/{Bucket}/{Key+}", "GetObject"),
            Route::new("GET", "/", "ListBuckets"),
            // Declared in the order the real S3 model lists them, so
            // `CopyObject` precedes `PutObject`; reverse them and `CopyObject`
            // hijacks every object write.
            Route::new("PUT", "/{Bucket}/{Key+}?x-id=CopyObject", "CopyObject")
                .with_required(COPY_SOURCE),
            Route::new("PUT", "/{Bucket}/{Key+}?x-id=PutObject", "PutObject"),
            Route::new("PUT", "/{Bucket}/{Key+}?x-id=UploadPart", "UploadPart")
                .with_required(PART_UPLOAD),
            Route::new(
                "PUT",
                "/{Bucket}/{Key+}?x-id=UploadPartCopy",
                "UploadPartCopy",
            )
            .with_required(PART_UPLOAD_COPY),
            Route::new(
                "DELETE",
                "/{Bucket}/{Key+}?x-id=AbortMultipartUpload",
                "AbortMultipartUpload",
            )
            .with_required(UPLOAD_ID),
            Route::new(
                "DELETE",
                "/{Bucket}/{Key+}?x-id=DeleteObject",
                "DeleteObject",
            ),
        ]
    }

    fn lookup(method: &str, uri: &str) -> Option<&'static str> {
        let routes = s3_routes();
        route_lookup(
            &routes,
            method,
            request_path(uri),
            request_query(uri),
            &no_headers,
        )
    }

    /// `lookup` with a set of request header names present.
    fn lookup_with_headers(method: &str, uri: &str, header_names: &[&str]) -> Option<&'static str> {
        let routes = s3_routes();
        let owned: Vec<String> = header_names.iter().map(|n| n.to_lowercase()).collect();
        route_lookup(
            &routes,
            method,
            request_path(uri),
            request_query(uri),
            &|name| owned.iter().any(|n| n == &name.to_lowercase()),
        )
    }

    #[test]
    fn query_literal_selects_v2() {
        assert_eq!(
            lookup("GET", "/mybucket?list-type=2"),
            Some("ListObjectsV2")
        );
    }

    #[test]
    fn query_pinned_wins_over_bare_template() {
        assert_eq!(
            lookup("GET", "/mybucket?list-type=2&prefix=a/b&max-keys=5"),
            Some("ListObjectsV2")
        );
    }

    #[test]
    fn presence_only_literal_routes_location_and_uploads() {
        assert_eq!(
            lookup("GET", "/mybucket?location"),
            Some("GetBucketLocation")
        );
        assert_eq!(
            lookup("GET", "/mybucket?location="),
            Some("GetBucketLocation")
        );
        assert_eq!(
            lookup("GET", "/mybucket?uploads"),
            Some("ListMultipartUploads")
        );
    }

    #[test]
    fn bare_template_when_no_matching_query() {
        assert_eq!(lookup("GET", "/mybucket"), Some("ListObjects"));
        // Unrelated query does not accidentally trip a discriminator.
        assert_eq!(lookup("GET", "/mybucket?prefix=a"), Some("ListObjects"));
    }

    #[test]
    fn value_mismatch_falls_back_to_bare() {
        assert_eq!(lookup("GET", "/mybucket?list-type=1"), Some("ListObjects"));
    }

    #[test]
    fn method_and_path_specificity_preserved() {
        assert_eq!(lookup("PUT", "/mybucket"), Some("CreateBucket"));
        assert_eq!(lookup("GET", "/mybucket/a/b/c.txt?x=1"), Some("GetObject"));
        assert_eq!(lookup("GET", "/"), Some("ListBuckets"));
        assert_eq!(lookup("DELETE", "/mybucket"), None);
    }

    #[test]
    fn no_query_route_table_is_a_pure_regression_check() {
        // A route table with NO query literals must behave identically to the
        // legacy method+path matcher.
        let routes = [
            Route::new("POST", "/2013-04-01/hostedzone", "CreateHostedZone"),
            Route::new("GET", "/2013-04-01/hostedzone/{Id}", "GetHostedZone"),
        ];
        assert_eq!(
            route_lookup(&routes, "POST", "/2013-04-01/hostedzone", "", &no_headers),
            Some("CreateHostedZone")
        );
        assert_eq!(
            route_lookup(
                &routes,
                "GET",
                "/2013-04-01/hostedzone/Z123",
                "",
                &no_headers
            ),
            Some("GetHostedZone")
        );
        // A stray query on a no-query route must not break matching.
        assert_eq!(
            route_lookup(
                &routes,
                "GET",
                "/2013-04-01/hostedzone/Z123",
                "foo=bar",
                &no_headers
            ),
            Some("GetHostedZone")
        );
        assert_eq!(route_lookup(&routes, "GET", "/nope", "", &no_headers), None);
    }

    // ------------------------------------------------------------------
    // Required-binding disambiguation (the S3 object-write collision).
    //
    // `PutObject`, `CopyObject`, `UploadPart` and `UploadPartCopy` all model
    // `PUT /{Bucket}/{Key+}` and differ ONLY by an `x-id` query literal that
    // AWS SDKs do not send. Routing must therefore fall back to the request's
    // shape, driven by each operation's `@required` header/query bindings.
    // ------------------------------------------------------------------

    /// A plain object write — exactly what boto3 puts on the wire — must reach
    /// `PutObject`. Before required-binding scoring, all four candidates tied at
    /// query score 0 and `CopyObject` won on iteration order, making `PutObject`
    /// unreachable and failing with "missing required member 'CopySource'".
    #[test]
    fn plain_object_put_without_x_id_routes_to_put_object() {
        assert_eq!(lookup("PUT", "/mybucket/a/b/c.txt"), Some("PutObject"));
    }

    /// The presence of `@required` `@httpHeader x-amz-copy-source` — the signal
    /// real S3 uses — selects `CopyObject` over `PutObject`.
    #[test]
    fn copy_source_header_routes_to_copy_object() {
        assert_eq!(
            lookup_with_headers("PUT", "/mybucket/dst.txt", &["x-amz-copy-source"]),
            Some("CopyObject")
        );
    }

    /// Header matching is case-insensitive on the wire; the probe supplied by
    /// the protocol looks the name up in the request's header map.
    #[test]
    fn copy_source_header_match_is_case_insensitive() {
        assert_eq!(
            lookup_with_headers("PUT", "/mybucket/dst.txt", &["X-Amz-Copy-Source"]),
            Some("CopyObject")
        );
    }

    /// `@required` `@httpQuery partNumber`+`uploadId` select `UploadPart`.
    #[test]
    fn part_number_and_upload_id_route_to_upload_part() {
        assert_eq!(
            lookup("PUT", "/mybucket/a.bin?partNumber=2&uploadId=abc"),
            Some("UploadPart")
        );
    }

    /// Satisfying MORE required bindings wins: copy-source + part bindings is
    /// `UploadPartCopy`, not `UploadPart` and not `CopyObject`.
    #[test]
    fn copy_source_plus_part_bindings_route_to_upload_part_copy() {
        assert_eq!(
            lookup_with_headers(
                "PUT",
                "/mybucket/a.bin?partNumber=2&uploadId=abc",
                &["x-amz-copy-source"]
            ),
            Some("UploadPartCopy")
        );
    }

    /// Negative case: a PARTIALLY satisfied required set must not select the
    /// route. `partNumber` alone leaves `uploadId` missing, so `UploadPart` is
    /// rejected outright and the write falls through to `PutObject`.
    #[test]
    fn partially_satisfied_required_bindings_reject_the_route() {
        assert_eq!(
            lookup("PUT", "/mybucket/a.bin?partNumber=2"),
            Some("PutObject")
        );
        assert_eq!(
            lookup("PUT", "/mybucket/a.bin?uploadId=abc"),
            Some("PutObject")
        );
    }

    /// The same collision on DELETE: `DeleteObject` vs `AbortMultipartUpload`.
    /// Without the `UploadId` separation `DeleteObject` loses to
    /// `AbortMultipartUpload` and fails with "missing required member 'UploadId'".
    #[test]
    fn delete_object_and_abort_multipart_upload_are_separated_by_upload_id() {
        assert_eq!(lookup("DELETE", "/mybucket/a.txt"), Some("DeleteObject"));
        assert_eq!(
            lookup("DELETE", "/mybucket/a.txt?uploadId=abc"),
            Some("AbortMultipartUpload")
        );
    }

    /// An explicit `x-id` (which newer SDKs DO send) still outranks the
    /// request-shape guess, because query score is compared before required
    /// score.
    #[test]
    fn explicit_x_id_outranks_required_binding_score() {
        assert_eq!(
            lookup("PUT", "/mybucket/a.txt?x-id=PutObject"),
            Some("PutObject")
        );
        assert_eq!(
            lookup_with_headers(
                "PUT",
                "/mybucket/a.txt?x-id=CopyObject",
                &["x-amz-copy-source"]
            ),
            Some("CopyObject")
        );
    }

    /// Negative case: a route whose required HEADER is absent is rejected even
    /// when its `x-id` literal matches, because a missing `@required` member
    /// makes the operation uninvokable.
    #[test]
    fn required_header_missing_rejects_route_despite_matching_x_id() {
        assert_eq!(
            lookup("PUT", "/mybucket/a.txt?x-id=CopyObject"),
            Some("PutObject")
        );
    }

    /// `required_binding_score` in isolation: all present → count, any missing
    /// → `None`, empty set → `Some(0)`.
    #[test]
    fn required_binding_score_semantics() {
        let query = parse_query_pairs("uploadId=abc");
        assert_eq!(required_binding_score(&[], &query, &no_headers), Some(0));
        assert_eq!(
            required_binding_score(UPLOAD_ID, &query, &no_headers),
            Some(1)
        );
        assert_eq!(
            required_binding_score(PART_UPLOAD, &query, &no_headers),
            None
        );
        assert_eq!(
            required_binding_score(COPY_SOURCE, &query, &no_headers),
            None
        );
        assert_eq!(
            required_binding_score(COPY_SOURCE, &query, &|n| n == "x-amz-copy-source"),
            Some(1)
        );
    }

    #[test]
    fn routing_key_end_to_end_through_protocol() {
        let proto = RestXmlServerProtocol::new(&s3_routes());
        let mut req = Request::new(SdkBody::empty());
        req.set_uri("/mybucket?list-type=2").unwrap();
        req.set_method("GET").unwrap();
        assert_eq!(proto.routing_key(&req).unwrap(), "ListObjectsV2");

        let mut loc = Request::new(SdkBody::empty());
        loc.set_uri("/mybucket?location").unwrap();
        loc.set_method("GET").unwrap();
        assert_eq!(proto.routing_key(&loc).unwrap(), "GetBucketLocation");

        let mut bare = Request::new(SdkBody::empty());
        bare.set_uri("/mybucket").unwrap();
        bare.set_method("GET").unwrap();
        assert_eq!(proto.routing_key(&bare).unwrap(), "ListObjects");
    }

    #[test]
    fn split_pattern_query_splits_path_and_query() {
        assert_eq!(split_pattern_query("/{Bucket}"), ("/{Bucket}", None));
        assert_eq!(
            split_pattern_query("/{Bucket}?list-type=2"),
            ("/{Bucket}", Some("list-type=2"))
        );
        assert_eq!(
            split_pattern_query("/{Bucket}?uploads&x=1"),
            ("/{Bucket}", Some("uploads&x=1"))
        );
    }

    #[test]
    fn query_match_score_semantics() {
        assert_eq!(query_match_score(None, "anything=1"), Some(0));
        assert_eq!(
            query_match_score(Some("list-type=2"), "list-type=2&x=1"),
            Some(1)
        );
        assert_eq!(query_match_score(Some("list-type=2"), "list-type=1"), None);
        assert_eq!(query_match_score(Some("list-type=2"), ""), None);
        assert_eq!(query_match_score(Some("location"), "location"), Some(1));
        assert_eq!(
            query_match_score(Some("location"), "location=us-west-2"),
            Some(1)
        );
        assert_eq!(query_match_score(Some("location"), "prefix=x"), None);
    }

    // =====================================================================
    // struct / union / Unit `@httpPayload` (this change)
    //
    // These lock the REST-XML side of the fix: a `structure`/`union` (and
    // list/map/document) `@httpPayload` body is parsed/serialized as the WHOLE
    // body via the XML codec, and a `smithy.api#Unit` output carries no body.
    // Before this change a struct payload hard-errored with "cannot read a
    // struct from a raw @httpPayload body" and a Unit output serialized a bogus
    // `<Unit></Unit>` element.
    // =====================================================================

    use aws_smithy_schema::serde::ShapeSerializer;
    use aws_smithy_schema::traits::HttpTrait;

    // --- PutBucketVersioning-style struct `@httpPayload` INPUT ---
    // PUT /{Bucket}?versioning
    //   Bucket                  String  @httpLabel
    //   VersioningConfiguration <struct> @httpPayload  (the XML body)
    static PBV_STATUS: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#VersioningConfiguration$Status",
            "test#VersioningConfiguration",
            "Status",
        ),
        ShapeType::String,
        "Status",
        0,
    );
    static PBV_CFG_MEMBERS: &[&Schema] = &[&PBV_STATUS];
    static PBV_CONFIG_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "test#VersioningConfiguration",
            "test",
            "VersioningConfiguration",
        ),
        ShapeType::Structure,
        PBV_CFG_MEMBERS,
    );
    static PBV_BUCKET: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#PutBucketVersioningRequest$Bucket",
            "test#PutBucketVersioningRequest",
            "Bucket",
        ),
        ShapeType::String,
        "Bucket",
        0,
    )
    .with_http_label();
    static PBV_PAYLOAD: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#PutBucketVersioningRequest$VersioningConfiguration",
            "test#PutBucketVersioningRequest",
            "VersioningConfiguration",
        ),
        ShapeType::Structure,
        "VersioningConfiguration",
        1,
    )
    .with_http_payload();
    static PBV_IN_MEMBERS: &[&Schema] = &[&PBV_BUCKET, &PBV_PAYLOAD];
    static PBV_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "test#PutBucketVersioningRequest",
            "test",
            "PutBucketVersioningRequest",
        ),
        ShapeType::Structure,
        PBV_IN_MEMBERS,
    )
    .with_http(HttpTrait::new("PUT", "/{Bucket}?versioning", None));

    #[test]
    fn deserialize_input_parses_struct_http_payload_from_xml_body() {
        let proto = RestXmlServerProtocol::new(&[Route::new(
            "PUT",
            "/{Bucket}?versioning",
            "PutBucketVersioning",
        )]);
        let body =
            b"<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>".to_vec();
        let mut req = Request::new(SdkBody::from(body));
        req.set_uri("/my-bucket?versioning").unwrap();
        req.set_method("PUT").unwrap();

        let mut de = proto
            .deserialize_input(&req, &PBV_INPUT_SCHEMA)
            .expect("deserialize_input");

        let mut bucket: Option<String> = None;
        let mut status: Option<String> = None;
        de.read_struct(&PBV_INPUT_SCHEMA, &mut |member, field| {
            match member.member_name() {
                Some("Bucket") => bucket = Some(field.read_string(member)?),
                Some("VersioningConfiguration") => {
                    // The struct @httpPayload: the whole XML body IS this
                    // shape. Previously this errored; now it parses.
                    field.read_struct(&PBV_CONFIG_SCHEMA, &mut |m, d| {
                        if m.member_name() == Some("Status") {
                            status = Some(d.read_string(m)?);
                        }
                        Ok(())
                    })?;
                }
                _ => {}
            }
            Ok(())
        })
        .expect("read_struct must succeed for a struct @httpPayload");

        assert_eq!(
            bucket.as_deref(),
            Some("my-bucket"),
            "@httpLabel Bucket must still bind from the URI alongside a struct payload"
        );
        assert_eq!(
            status.as_deref(),
            Some("Enabled"),
            "struct @httpPayload body must parse into the target struct's members"
        );
    }

    // --- List `@httpPayload` INPUT (Delete/DeleteObjects-style) ---
    static DEL_KEY_ITEM: Schema = Schema::new_member(
        ShapeId::from_static("test#Delete$Object", "test#Delete", "Object"),
        ShapeType::String,
        "Object",
        0,
    );
    static DEL_MEMBERS: &[&Schema] = &[&DEL_KEY_ITEM];
    static DEL_LIST_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#Delete", "test", "Delete"),
        ShapeType::Structure,
        DEL_MEMBERS,
    );
    static DEL_PAYLOAD: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#DeleteObjectsRequest$Delete",
            "test#DeleteObjectsRequest",
            "Delete",
        ),
        ShapeType::Structure,
        "Delete",
        0,
    )
    .with_http_payload();
    static DEL_IN_MEMBERS: &[&Schema] = &[&DEL_PAYLOAD];
    static DEL_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#DeleteObjectsRequest", "test", "DeleteObjectsRequest"),
        ShapeType::Structure,
        DEL_IN_MEMBERS,
    );

    #[test]
    fn deserialize_input_parses_nested_struct_payload_members() {
        let proto = RestXmlServerProtocol::new(&[]);
        let body = b"<Delete><Object>a.txt</Object></Delete>".to_vec();
        let mut req = Request::new(SdkBody::from(body));
        req.set_uri("/my-bucket?delete").unwrap();
        req.set_method("POST").unwrap();

        let mut de = proto
            .deserialize_input(&req, &DEL_INPUT_SCHEMA)
            .expect("deserialize_input");

        let mut object: Option<String> = None;
        de.read_struct(&DEL_INPUT_SCHEMA, &mut |member, field| {
            if member.member_name() == Some("Delete") {
                field.read_struct(&DEL_LIST_SCHEMA, &mut |m, d| {
                    if m.member_name() == Some("Object") {
                        object = Some(d.read_string(m)?);
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })
        .expect("read_struct");
        assert_eq!(object.as_deref(), Some("a.txt"));
    }

    // --- CopyObject-style struct `@httpPayload` OUTPUT (CopyObjectResult) ---
    static COR_ETAG: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CopyObjectResult$ETag",
            "test#CopyObjectResult",
            "ETag",
        ),
        ShapeType::String,
        "ETag",
        0,
    );
    static COR_MEMBERS: &[&Schema] = &[&COR_ETAG];
    static COR_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#CopyObjectResult", "test", "CopyObjectResult"),
        ShapeType::Structure,
        COR_MEMBERS,
    );
    static COO_PAYLOAD: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CopyObjectOutput$CopyObjectResult",
            "test#CopyObjectOutput",
            "CopyObjectResult",
        ),
        ShapeType::Structure,
        "CopyObjectResult",
        0,
    )
    .with_http_payload();
    static COO_MEMBERS: &[&Schema] = &[&COO_PAYLOAD];
    static COO_OUTPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#CopyObjectOutput", "test", "CopyObjectOutput"),
        ShapeType::Structure,
        COO_MEMBERS,
    );

    struct CopyObjectResultValue {
        etag: String,
    }
    impl SerializableStruct for CopyObjectResultValue {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_string(&COR_ETAG, &self.etag)?;
            let _ = &COR_SCHEMA;
            Ok(())
        }
    }
    struct CopyObjectOutputValue {
        result: CopyObjectResultValue,
    }
    impl SerializableStruct for CopyObjectOutputValue {
        fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            s.write_struct(&COO_PAYLOAD, &self.result)
        }
    }

    #[test]
    fn serialize_output_emits_struct_http_payload_as_whole_xml_body() {
        let proto = RestXmlServerProtocol::new(&[]);
        let out = CopyObjectOutputValue {
            result: CopyObjectResultValue {
                etag: "abc123".to_string(),
            },
        };
        let resp = proto
            .serialize_output(&out, &COO_OUTPUT_SCHEMA, "TestOp", 200)
            .expect("serialize_output");

        assert_eq!(resp.status().as_u16(), 200);
        assert_eq!(resp.headers().get("content-type"), Some("application/xml"));
        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        assert!(
            body.starts_with("<CopyObjectResult"),
            "struct @httpPayload output must be its OWN root element; got: {body}"
        );
        assert!(
            body.contains("<ETag>abc123</ETag>"),
            "payload struct members must serialize as XML elements; got: {body}"
        );
        assert!(
            !body.contains("<CopyObjectOutput"),
            "a struct @httpPayload must NOT be wrapped in the <Output> envelope; got: {body}"
        );
    }

    // --- union `@httpPayload` INPUT ---
    static UNI_TEXT: Schema = Schema::new_member(
        ShapeId::from_static("test#Choice$Text", "test#Choice", "Text"),
        ShapeType::String,
        "Text",
        0,
    );
    static UNI_MEMBERS: &[&Schema] = &[&UNI_TEXT];
    static UNI_UNION_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#Choice", "test", "Choice"),
        ShapeType::Union,
        UNI_MEMBERS,
    );
    static UNI_PAYLOAD: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#PutChoiceRequest$Choice",
            "test#PutChoiceRequest",
            "Choice",
        ),
        ShapeType::Union,
        "Choice",
        0,
    )
    .with_http_payload();
    static UNI_IN_MEMBERS: &[&Schema] = &[&UNI_PAYLOAD];
    static UNI_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#PutChoiceRequest", "test", "PutChoiceRequest"),
        ShapeType::Structure,
        UNI_IN_MEMBERS,
    );

    #[test]
    fn deserialize_input_parses_union_http_payload_variant() {
        let proto = RestXmlServerProtocol::new(&[]);
        let body = b"<Choice><Text>hello</Text></Choice>".to_vec();
        let mut req = Request::new(SdkBody::from(body));
        req.set_uri("/choice").unwrap();
        req.set_method("PUT").unwrap();

        let mut de = proto
            .deserialize_input(&req, &UNI_INPUT_SCHEMA)
            .expect("deserialize_input");

        let mut text: Option<String> = None;
        de.read_struct(&UNI_INPUT_SCHEMA, &mut |member, field| {
            if member.member_name() == Some("Choice") {
                // A union payload is read like a struct: the present variant is
                // the single child element.
                field.read_struct(&UNI_UNION_SCHEMA, &mut |m, d| {
                    if m.member_name() == Some("Text") {
                        text = Some(d.read_string(m)?);
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })
        .expect("read_struct");
        assert_eq!(
            text.as_deref(),
            Some("hello"),
            "union @httpPayload must parse the active variant from the XML body"
        );
    }

    // --- Unit output: empty body, no bogus <Unit/> element ---
    static UNIT_NO_CODE_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ShapeType::Structure,
        &[],
    );
    static UNIT_204_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ShapeType::Structure,
        &[],
    );

    struct UnitValue;
    impl SerializableStruct for UnitValue {
        fn serialize_members(&self, _s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
            Ok(())
        }
    }

    #[test]
    fn serialize_output_unit_emits_empty_body_not_bogus_unit_element() {
        let proto = RestXmlServerProtocol::new(&[])
            .with_namespace("http://s3.amazonaws.com/doc/2006-03-01/");
        let resp = proto
            .serialize_output(&UnitValue, &UNIT_NO_CODE_SCHEMA, "TestOp", 200)
            .expect("serialize_output");
        assert!(
            resp.body().bytes().unwrap().is_empty(),
            "a smithy.api#Unit output must have an EMPTY body, not a bogus <Unit></Unit> element"
        );
    }

    #[test]
    fn serialize_output_unit_honors_operation_http_code() {
        let proto = RestXmlServerProtocol::new(&[])
            .with_namespace("http://s3.amazonaws.com/doc/2006-03-01/");
        let resp = proto
            .serialize_output(&UnitValue, &UNIT_204_SCHEMA, "TestOp", 204)
            .expect("serialize_output");
        assert!(resp.body().bytes().unwrap().is_empty());
        assert_eq!(
            resp.status().as_u16(),
            204,
            "the operation's success status must be the response status (e.g. 204)"
        );
    }

    // --- regression: blob `@httpPayload` INPUT still binds raw bytes ---
    static BLOB_KEY: Schema = Schema::new_member(
        ShapeId::from_static("test#PutObjectRequest$Key", "test#PutObjectRequest", "Key"),
        ShapeType::String,
        "Key",
        0,
    )
    .with_http_label();
    static BLOB_BODY: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#PutObjectRequest$Body",
            "test#PutObjectRequest",
            "Body",
        ),
        ShapeType::Blob,
        "Body",
        1,
    )
    .with_http_payload();
    static BLOB_IN_MEMBERS: &[&Schema] = &[&BLOB_KEY, &BLOB_BODY];
    static BLOB_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#PutObjectRequest", "test", "PutObjectRequest"),
        ShapeType::Structure,
        BLOB_IN_MEMBERS,
    )
    .with_http(HttpTrait::new("PUT", "/{Key+}", None));

    #[test]
    fn deserialize_input_blob_http_payload_still_binds_raw_bytes() {
        let proto = RestXmlServerProtocol::new(&[Route::new("PUT", "/{Key+}", "PutObject")]);
        // Bytes that would break an XML parser must bind verbatim.
        let raw = b"\x00<not-xml>{&\xff raw".to_vec();
        let mut req = Request::new(SdkBody::from(raw.clone()));
        req.set_uri("/obj.bin").unwrap();
        req.set_method("PUT").unwrap();

        let mut de = proto
            .deserialize_input(&req, &BLOB_INPUT_SCHEMA)
            .expect("deserialize_input");

        let mut body: Option<aws_smithy_types::Blob> = None;
        de.read_struct(&BLOB_INPUT_SCHEMA, &mut |member, field| {
            if member.member_name() == Some("Body") {
                body = Some(field.read_blob(member)?);
            }
            Ok(())
        })
        .expect("read_struct");
        assert_eq!(
            body.expect("body").as_ref(),
            raw.as_slice(),
            "blob @httpPayload must still bind the raw body verbatim"
        );
    }

    // --- S3 CreateBucket-style OPTIONAL struct `@httpPayload` INPUT ---
    // PUT /{Bucket}
    //   Bucket                    String   @httpLabel
    //   CreateBucketConfiguration <struct> @httpPayload  (optional)
    static CB_LOCATION: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateBucketConfiguration$LocationConstraint",
            "test#CreateBucketConfiguration",
            "LocationConstraint",
        ),
        ShapeType::String,
        "LocationConstraint",
        0,
    );
    static CB_CFG_MEMBERS: &[&Schema] = &[&CB_LOCATION];
    static CB_CONFIG_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static(
            "test#CreateBucketConfiguration",
            "test",
            "CreateBucketConfiguration",
        ),
        ShapeType::Structure,
        CB_CFG_MEMBERS,
    );
    static CB_BUCKET: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateBucketRequest$Bucket",
            "test#CreateBucketRequest",
            "Bucket",
        ),
        ShapeType::String,
        "Bucket",
        0,
    )
    .with_http_label();
    static CB_PAYLOAD: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#CreateBucketRequest$CreateBucketConfiguration",
            "test#CreateBucketRequest",
            "CreateBucketConfiguration",
        ),
        ShapeType::Structure,
        "CreateBucketConfiguration",
        1,
    )
    .with_http_payload();
    static CB_IN_MEMBERS: &[&Schema] = &[&CB_BUCKET, &CB_PAYLOAD];
    static CB_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#CreateBucketRequest", "test", "CreateBucketRequest"),
        ShapeType::Structure,
        CB_IN_MEMBERS,
    )
    .with_http(HttpTrait::new("PUT", "/{Bucket}", None));

    /// Drive `PUT /my-bucket` with `raw_body` all the way through
    /// `deserialize_input` + `read_struct`, yielding the `Bucket` @httpLabel and
    /// the payload's `LocationConstraint` as the member consumer observed them.
    fn create_bucket_roundtrip(raw_body: &[u8]) -> (Option<String>, Option<String>) {
        let proto = RestXmlServerProtocol::new(&[Route::new("PUT", "/{Bucket}", "CreateBucket")]);
        let mut req = Request::new(SdkBody::from(raw_body.to_vec()));
        req.set_uri("/my-bucket").unwrap();
        req.set_method("PUT").unwrap();

        let mut de = proto
            .deserialize_input(&req, &CB_INPUT_SCHEMA)
            .expect("deserialize_input must accept every CreateBucket body shape");

        let mut bucket: Option<String> = None;
        let mut location: Option<String> = None;
        de.read_struct(&CB_INPUT_SCHEMA, &mut |member, field| {
            match member.member_name() {
                Some("Bucket") => bucket = Some(field.read_string(member)?),
                Some("CreateBucketConfiguration") => {
                    field.read_struct(&CB_CONFIG_SCHEMA, &mut |m, d| {
                        if m.member_name() == Some("LocationConstraint") {
                            location = Some(d.read_string(m)?);
                        }
                        Ok(())
                    })?;
                }
                _ => {}
            }
            Ok(())
        })
        .expect("read_struct must succeed for every CreateBucket body shape");
        (bucket, location)
    }

    #[test]
    fn create_bucket_empty_body_binds_label_and_leaves_payload_absent() {
        let (bucket, location) = create_bucket_roundtrip(b"");
        assert_eq!(
            bucket.as_deref(),
            Some("my-bucket"),
            "@httpLabel must still bind when the optional payload body is omitted"
        );
        assert_eq!(
            location, None,
            "an omitted optional @httpPayload must deserialize as absent"
        );
    }

    #[test]
    fn create_bucket_whitespace_only_body_is_treated_as_absent_payload() {
        // Some SDKs/proxies pad an omitted body with a newline. That is not an
        // XML document, so it must never reach the parser.
        let (bucket, location) = create_bucket_roundtrip(b"\n  \t\r\n");
        assert_eq!(bucket.as_deref(), Some("my-bucket"));
        assert_eq!(
            location, None,
            "a whitespace-only body must be treated as an omitted payload, not as invalid XML"
        );
    }

    #[test]
    fn create_bucket_declaration_only_body_is_treated_as_empty_document() {
        // Non-blank bytes, so this *does* reach the XML parser, but it holds no
        // root element. That is a zero-member struct, not a parse failure.
        let (bucket, location) =
            create_bucket_roundtrip(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
        assert_eq!(bucket.as_deref(), Some("my-bucket"));
        assert_eq!(
            location, None,
            "an element-less XML document must yield no payload members"
        );
    }

    #[test]
    fn create_bucket_self_closing_payload_root_yields_no_members() {
        // `<CreateBucketConfiguration/>` surfaces as an `Empty` event, never a
        // `Start`, so the root consumer has to recognise it.
        let (bucket, location) = create_bucket_roundtrip(b"<CreateBucketConfiguration/>");
        assert_eq!(bucket.as_deref(), Some("my-bucket"));
        assert_eq!(
            location, None,
            "a childless self-closing payload root must yield no members"
        );
    }

    #[test]
    fn create_bucket_populated_payload_still_parses() {
        // Guards the happy path: tolerating blank bodies must not stop real
        // payloads from binding.
        let (bucket, location) = create_bucket_roundtrip(
            b"<CreateBucketConfiguration><LocationConstraint>eu-west-1</LocationConstraint></CreateBucketConfiguration>",
        );
        assert_eq!(bucket.as_deref(), Some("my-bucket"));
        assert_eq!(
            location.as_deref(),
            Some("eu-west-1"),
            "a populated @httpPayload must still parse into its members"
        );
    }

    // --- DeleteBucket-style INPUT: @httpLabel only, no `@httpPayload` ---
    static DB_BUCKET: Schema = Schema::new_member(
        ShapeId::from_static(
            "test#DeleteBucketRequest$Bucket",
            "test#DeleteBucketRequest",
            "Bucket",
        ),
        ShapeType::String,
        "Bucket",
        0,
    )
    .with_http_label();
    static DB_IN_MEMBERS: &[&Schema] = &[&DB_BUCKET];
    static DB_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#DeleteBucketRequest", "test", "DeleteBucketRequest"),
        ShapeType::Structure,
        DB_IN_MEMBERS,
    )
    .with_http(HttpTrait::new("DELETE", "/{Bucket}", None));

    #[test]
    fn whitespace_only_body_on_payload_free_input_still_binds_labels() {
        let proto =
            RestXmlServerProtocol::new(&[Route::new("DELETE", "/{Bucket}", "DeleteBucket")]);
        let mut req = Request::new(SdkBody::from(b"\r\n \t".to_vec()));
        req.set_uri("/my-bucket").unwrap();
        req.set_method("DELETE").unwrap();

        let mut de = proto
            .deserialize_input(&req, &DB_INPUT_SCHEMA)
            .expect("deserialize_input must accept a whitespace-only body");

        let mut bucket: Option<String> = None;
        de.read_struct(&DB_INPUT_SCHEMA, &mut |member, field| {
            if member.member_name() == Some("Bucket") {
                bucket = Some(field.read_string(member)?);
            }
            Ok(())
        })
        .expect("read_struct must succeed for a whitespace-only body");
        assert_eq!(
            bucket.as_deref(),
            Some("my-bucket"),
            "a whitespace-only body must not stop @httpLabel members from binding"
        );
    }

    // =====================================================================
    // scalar `@httpPayload`: whitespace IS data (this change)
    //
    // The blank-body short-circuit above exists because a blank body holds no
    // XML *document*. A scalar (`blob`/`string`) `@httpPayload` body is not a
    // document — it is the member's literal value — so XML-insignificant
    // whitespace is significant data there. Before this change a body of `"   "`
    // was routed to `EmptyDeserializer` with no `with_payload_bytes`, so
    // `PayloadValueDeserializer` fell back to `unwrap_or_default()` and the
    // member silently deserialized as empty: three spaces stored as zero bytes.
    // =====================================================================

    // --- PutObject-style STRING `@httpPayload` INPUT ---
    static STR_KEY: Schema = Schema::new_member(
        ShapeId::from_static("test#PutTextRequest$Key", "test#PutTextRequest", "Key"),
        ShapeType::String,
        "Key",
        0,
    )
    .with_http_label();
    static STR_BODY: Schema = Schema::new_member(
        ShapeId::from_static("test#PutTextRequest$Body", "test#PutTextRequest", "Body"),
        ShapeType::String,
        "Body",
        1,
    )
    .with_http_payload();
    static STR_IN_MEMBERS: &[&Schema] = &[&STR_KEY, &STR_BODY];
    static STR_INPUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#PutTextRequest", "test", "PutTextRequest"),
        ShapeType::Structure,
        STR_IN_MEMBERS,
    )
    .with_http(HttpTrait::new("PUT", "/{Key+}", None));

    /// Drive `PUT /obj.txt` with `raw_body` through `deserialize_input` +
    /// `read_struct`, yielding the `Key` @httpLabel and the string
    /// `@httpPayload` as the member consumer observed them.
    fn put_text_roundtrip(raw_body: &[u8]) -> (Option<String>, Option<String>) {
        let proto = RestXmlServerProtocol::new(&[Route::new("PUT", "/{Key+}", "PutText")]);
        let mut req = Request::new(SdkBody::from(raw_body.to_vec()));
        req.set_uri("/obj.txt").unwrap();
        req.set_method("PUT").unwrap();

        let mut de = proto
            .deserialize_input(&req, &STR_INPUT_SCHEMA)
            .expect("deserialize_input must accept every scalar payload body");

        let mut key: Option<String> = None;
        let mut body: Option<String> = None;
        de.read_struct(&STR_INPUT_SCHEMA, &mut |member, field| {
            match member.member_name() {
                Some("Key") => key = Some(field.read_string(member)?),
                Some("Body") => body = Some(field.read_string(member)?),
                _ => {}
            }
            Ok(())
        })
        .expect("read_struct must succeed for every scalar payload body");
        (key, body)
    }

    #[test]
    fn whitespace_only_body_binds_verbatim_to_string_http_payload() {
        let (key, body) = put_text_roundtrip(b"   ");
        assert_eq!(key.as_deref(), Some("obj.txt"));
        assert_eq!(
            body.as_deref(),
            Some("   "),
            "a whitespace-only body is the LITERAL value of a scalar @httpPayload, \
             not an absent payload"
        );
    }

    #[test]
    fn newline_padded_body_binds_verbatim_to_string_http_payload() {
        // The exact byte pattern the blank-body short-circuit was written for:
        // significant here, because nothing parses this body as XML.
        let (key, body) = put_text_roundtrip(b"\n  \t\r\n");
        assert_eq!(key.as_deref(), Some("obj.txt"));
        assert_eq!(
            body.as_deref(),
            Some("\n  \t\r\n"),
            "every byte of a scalar @httpPayload body must survive deserialization"
        );
    }

    #[test]
    fn empty_body_still_binds_as_empty_string_http_payload() {
        // Pins the pre-existing behavior: an empty body carries no bytes, so the
        // payload is empty either way. Narrowing the blank check must not change
        // this.
        let (key, body) = put_text_roundtrip(b"");
        assert_eq!(key.as_deref(), Some("obj.txt"));
        assert_eq!(
            body.as_deref(),
            Some(""),
            "an empty body must bind a scalar @httpPayload as empty"
        );
    }

    #[test]
    fn whitespace_only_body_binds_verbatim_to_blob_http_payload() {
        let proto = RestXmlServerProtocol::new(&[Route::new("PUT", "/{Key+}", "PutObject")]);
        let raw = b" \t\n".to_vec();
        let mut req = Request::new(SdkBody::from(raw.clone()));
        req.set_uri("/obj.bin").unwrap();
        req.set_method("PUT").unwrap();

        let mut de = proto
            .deserialize_input(&req, &BLOB_INPUT_SCHEMA)
            .expect("deserialize_input must accept a whitespace-only blob payload");

        let mut body: Option<aws_smithy_types::Blob> = None;
        de.read_struct(&BLOB_INPUT_SCHEMA, &mut |member, field| {
            if member.member_name() == Some("Body") {
                body = Some(field.read_blob(member)?);
            }
            Ok(())
        })
        .expect("read_struct must succeed for a whitespace-only blob payload");
        assert_eq!(
            body.expect("body").as_ref(),
            raw.as_slice(),
            "a whitespace-only blob @httpPayload must bind verbatim — a 3-byte \
             object is not a 0-byte object"
        );
    }

    #[test]
    fn is_structured_payload_classifies_document_shapes_only() {
        // The classification that decides whether the blank-body short-circuit
        // applies. It must agree exactly with
        // `HttpBindingDeserializer::read_struct`: only these five shapes are
        // parsed by the body codec.
        assert!(
            is_structured_payload(&CB_PAYLOAD),
            "structure is a document"
        );
        for structured in [
            ShapeType::Structure,
            ShapeType::Union,
            ShapeType::List,
            ShapeType::Map,
            ShapeType::Document,
        ] {
            assert!(
                is_structured_payload(&member_of_type(structured)),
                "{structured:?} payloads are parsed as a whole XML document"
            );
        }
        for scalar in [
            ShapeType::Blob,
            ShapeType::String,
            ShapeType::Integer,
            ShapeType::Boolean,
        ] {
            assert!(
                !is_structured_payload(&member_of_type(scalar)),
                "{scalar:?} payloads bind the raw body verbatim"
            );
        }
    }

    /// A throwaway payload member schema of the given target shape, for
    /// classification tests.
    fn member_of_type(shape_type: ShapeType) -> Schema {
        Schema::new_member(
            ShapeId::from_static("test#Req$Payload", "test#Req", "Payload"),
            shape_type,
            "Payload",
            0,
        )
        .with_http_payload()
    }

    // =====================================================================
    // `noErrorWrapping` error envelope
    //
    // A restXml service whose model sets `noErrorWrapping` must answer errors
    // with a bare `<Error>` root (no `<ErrorResponse>` wrapper, no `<Type>`,
    // trailing `<RequestId>`/`<HostId>`) plus the `x-amz-request-id` /
    // `x-amz-id-2` headers. The two ids are generated per response — the body
    // element and its matching header carry the same value, and a request-id is
    // a well-formed v4 UUID while a host-id is 64 lowercase hex characters. The
    // default (flag absent) stays the wrapped `<ErrorResponse><Error><Type>…`
    // envelope.
    // =====================================================================

    /// Assert `s` is a canonical v4 UUID: `8-4-4-4-12` lowercase hex, version
    /// nibble `4`, and variant nibble one of `8`/`9`/`a`/`b`.
    fn assert_is_v4_uuid(s: &str) {
        let groups: Vec<&str> = s.split('-').collect();
        assert_eq!(groups.len(), 5, "a UUID has five groups: {s}");
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "group lengths must be 8-4-4-4-12: {s}"
        );
        assert!(
            s.chars()
                .all(|c| c == '-' || c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "a UUID is lowercase hex and hyphens: {s}"
        );
        // Third group's first nibble is the version (4); fourth group's first
        // nibble is the variant (8/9/a/b).
        assert_eq!(&groups[2][0..1], "4", "version nibble must be 4: {s}");
        assert!(
            matches!(&groups[3][0..1], "8" | "9" | "a" | "b"),
            "variant nibble must be 8/9/a/b: {s}"
        );
    }

    /// Assert `s` is the S3 `x-amz-id-2` shape: exactly 64 lowercase hex chars.
    fn assert_is_host_id(s: &str) {
        assert_eq!(s.len(), 64, "a host-id is 64 hex characters: {s}");
        assert!(
            s.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "a host-id is lowercase hex: {s}"
        );
    }

    // A modeled error whose only body member is `Message` — the shape every
    // restXml error carries. `error_code` and the envelope are supplied by
    // `serialize_error`; this struct contributes just the member.
    static ERR_MESSAGE_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("test#NoSuchKey$Message", "test#NoSuchKey", "Message"),
        ShapeType::String,
        "Message",
        0,
    );
    struct NoSuchKeyError {
        message: String,
    }
    impl SerializableStruct for NoSuchKeyError {
        fn serialize_members(
            &self,
            s: &mut dyn aws_smithy_schema::serde::ShapeSerializer,
        ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
            s.write_string(&ERR_MESSAGE_MEMBER, &self.message)
        }
    }
    static NO_SUCH_KEY_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("test#NoSuchKey", "test", "NoSuchKey"),
        ShapeType::Structure,
        &[&ERR_MESSAGE_MEMBER],
    );

    fn no_such_key() -> NoSuchKeyError {
        NoSuchKeyError {
            message: "The specified key does not exist.".to_string(),
        }
    }

    /// Pull the text of the first `<Element>text</Element>` out of `body`.
    fn element_text<'a>(body: &'a str, element: &str) -> &'a str {
        let open = format!("<{element}>");
        let close = format!("</{element}>");
        let start = body
            .find(&open)
            .unwrap_or_else(|| panic!("<{element}> in {body}"))
            + open.len();
        let end = body[start..]
            .find(&close)
            .unwrap_or_else(|| panic!("</{element}> in {body}"))
            + start;
        &body[start..end]
    }

    #[test]
    fn serialize_error_unwrapped_emits_bare_error_root_and_matching_request_id() {
        let proto = RestXmlServerProtocol::new(&[]).with_no_error_wrapping(true);
        let resp = proto
            .serialize_error(&no_such_key(), &NO_SUCH_KEY_SCHEMA, "NoSuchKey", 404)
            .expect("serialize_error");

        assert_eq!(resp.status().as_u16(), 404);
        assert_eq!(resp.headers().get("content-type"), Some("application/xml"));

        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        // Bare <Error> root — no wrapper, no <Type>.
        assert!(
            body.starts_with("<Error><Code>NoSuchKey</Code>"),
            "unwrapped error must be a bare <Error> root with no <ErrorResponse> \
             and no <Type>; got: {body}"
        );
        assert!(!body.contains("<ErrorResponse"), "{body}");
        assert!(!body.contains("<Type>"), "{body}");
        assert!(
            body.contains("<Message>The specified key does not exist.</Message>"),
            "{body}"
        );

        // The ids are well-formed and the header equals the body element.
        let body_request_id = element_text(body, "RequestId");
        let body_host_id = element_text(body, "HostId");
        assert_is_v4_uuid(body_request_id);
        assert_is_host_id(body_host_id);
        assert_eq!(
            resp.headers().get("x-amz-request-id"),
            Some(body_request_id),
            "x-amz-request-id header must equal the body's <RequestId>: {body}"
        );
        assert_eq!(
            resp.headers().get("x-amz-id-2"),
            Some(body_host_id),
            "x-amz-id-2 header must equal the body's <HostId>: {body}"
        );
    }

    #[test]
    fn serialize_error_default_keeps_the_wrapped_envelope_and_no_request_id_headers() {
        // The flag absent (the default): the envelope must be unchanged from the
        // Smithy restXml default, and must carry no request-id headers.
        let proto = RestXmlServerProtocol::new(&[]);
        let resp = proto
            .serialize_error(&no_such_key(), &NO_SUCH_KEY_SCHEMA, "NoSuchKey", 404)
            .expect("serialize_error");

        assert_eq!(resp.status().as_u16(), 404);
        assert_eq!(resp.headers().get("content-type"), Some("application/xml"));
        assert_eq!(
            resp.headers().get("x-amz-request-id"),
            None,
            "the wrapped envelope must not add request-id headers"
        );
        assert_eq!(resp.headers().get("x-amz-id-2"), None);

        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        assert_eq!(
            body,
            "<ErrorResponse><Error><Type>Sender</Type>\
             <Code>NoSuchKey</Code>\
             <Message>The specified key does not exist.</Message></Error></ErrorResponse>",
            "the default error envelope must stay wrapped and unchanged; got: {body}"
        );
    }

    #[test]
    fn serialize_error_unwrapped_is_a_type_free_root_even_on_a_5xx() {
        // The wrapped envelope flips <Type> to Receiver on a 5xx; the unwrapped
        // shape has no <Type> at all, so a server-fault status must not
        // reintroduce one.
        let proto = RestXmlServerProtocol::new(&[]).with_no_error_wrapping(true);
        let resp = proto
            .serialize_error(&no_such_key(), &NO_SUCH_KEY_SCHEMA, "InternalError", 500)
            .expect("serialize_error");
        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        assert!(
            !body.contains("<Type>"),
            "an unwrapped error must carry no <Type>, even on a 5xx: {body}"
        );
        assert!(body.starts_with("<Error>"), "{body}");
    }

    #[test]
    fn serialize_error_unwrapped_emits_bare_error_root_with_no_xmlns() {
        // A service whose model declares a namespace still must NOT stamp it on
        // the unwrapped `<Error>` bare root — S3 SDK error parsers read `Code`
        // from a namespace-free `<Error>` and find nothing when `xmlns` is
        // present. The wrapped `<ErrorResponse xmlns="...">` envelope keeps its
        // namespace.
        let proto = RestXmlServerProtocol::new(&[])
            .with_namespace("http://s3.amazonaws.com/doc/2006-03-01/")
            .with_no_error_wrapping(true);
        let resp = proto
            .serialize_error(&no_such_key(), &NO_SUCH_KEY_SCHEMA, "NoSuchKey", 404)
            .expect("serialize_error");

        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        assert!(
            body.starts_with("<Error><Code>NoSuchKey</Code>"),
            "unwrapped error must be a bare <Error> root with NO xmlns attribute; got: {body}"
        );
        assert!(
            !body.contains("xmlns="),
            "unwrapped error must carry NO namespace on the bare <Error> root; got: {body}"
        );
    }

    #[test]
    fn serialize_error_wrapped_keeps_namespace_on_error_response_envelope() {
        // The wrapped envelope with a service namespace must keep the `xmlns`
        // attribute on the `<ErrorResponse>` root.
        let proto = RestXmlServerProtocol::new(&[])
            .with_namespace("http://s3.amazonaws.com/doc/2006-03-01/");
        let resp = proto
            .serialize_error(&no_such_key(), &NO_SUCH_KEY_SCHEMA, "NoSuchKey", 404)
            .expect("serialize_error");

        let body = std::str::from_utf8(resp.body().bytes().unwrap()).unwrap();
        assert!(
            body.starts_with("<ErrorResponse xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">"),
            "wrapped envelope must carry the service namespace on <ErrorResponse>; got: {body}"
        );
        assert!(
            body.contains("<Error><Type>Sender</Type><Code>NoSuchKey</Code>"),
            "wrapped envelope structure must be unchanged; got: {body}"
        );
    }
}
