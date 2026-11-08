// SPDX-License-Identifier: CC0-1.0

//! The X-RAY TRACE CONTEXT a stage propagates onto the call its integration makes, and the
//! SEGMENT a tracing stage records for the request it served.
//!
//! A request arriving at a deployed stage is one hop of a request chain, and the backend the
//! stage calls is the next: X-Ray names that chain in the `X-Amzn-Trace-Id` header, so the
//! call API Gateway makes onward carries the chain the caller named, the span of the stage's
//! own call out as the parent the next hop's work hangs from, and the sampling decision the
//! caller took. Those decisions are `service_common`'s one propagation
//! ([`::service_common::xray_trace::onward_tracing_header`]) rather than this service's own,
//! because every hop of a chain takes them the same way:
//!
//! * a caller that supplied a `Root` in X-Ray's own form has that chain CONTINUED, and one
//!   that supplied none — or supplied text that is not a trace id — has the stage start a
//!   chain, so a malformed header never fails the request;
//! * the `Parent` is a span id minted for THIS call, because the stage's work is what the
//!   backend's work is a child of;
//! * the `Sampled` decision is the caller's where it took one, and the stage's own where it
//!   did not.
//!
//! WHICH of those two a stage does is decided by one record: `Stage$tracingEnabled`,
//! "specifies whether active tracing with X-ray is enabled for the Stage". A stage that is
//! tracing RECORDS the request — so it stamps the header, and submits a segment of its own
//! ([`stage_segment`]) whose `origin` is the stage resource type and which carries the call
//! out to the backend as a nested aws-namespace subsegment, the span the backend's own
//! segment then names as its parent. A stage that is NOT tracing records nothing: it has no
//! span to name and no decision to take, so the caller's own header travels on as written and
//! no segment is submitted, which is what makes the backend's own segment the root of such a
//! trace.
//!
//! Nothing here is kept: the context and the segment are built per call from the request, the
//! stage's own record and what the integration answered, and the segment is submitted through
//! the one in-process X-Ray client this tree has
//! ([`::service_common::xray_segments`]) — off the answer, so a submission that could not be
//! made never changes what the client is served.

use std::time::SystemTime;

use ::serde_json::{json, Value};

use super::lambda_proxy::ProxyInputs;

/// The header a tracing header travels on, as X-Ray spells it — the spelling the next hop
/// reads it back off ([`::service_common::xray_trace`]).
pub(super) const TRACE_HEADER: &str = "X-Amzn-Trace-Id";

/// The `origin` a segment a REST API stage recorded carries: the resource type of the stage,
/// as X-Ray names an API Gateway stage.
const STAGE_ORIGIN: &str = "AWS::ApiGateway::Stage";

/// The `name` of the subsegment covering the stage's call out to a Lambda integration: the
/// service called, which is how an aws-namespace subsegment names its downstream.
const LAMBDA_SUBSEGMENT_NAME: &str = "Lambda";

/// The `namespace` that makes a subsegment a call out through the AWS SDK, so X-Ray reads
/// the work it covers as an AWS service's — which is what has it SYNTHESISE the downstream
/// node for that service.
const AWS_NAMESPACE: &str = "aws";

/// What this stage propagates onto the call it makes onward, and what it records about the
/// request it served.
#[derive(Debug, Default, Clone)]
pub(super) struct Propagated {
    /// The tracing header the INTEGRATION's call carries, or `None` where the request named no
    /// chain and this stage started none.
    pub header: Option<String>,
    /// The tracing header a `CUSTOM` AUTHORIZER's invocation carries. The same chain and the
    /// same decision as [`Self::header`] — both calls are this one request's work — under the
    /// stage's OWN span, because an authorizer's function is called by the stage itself rather
    /// than through the integration's call out.
    pub authorizer_header: Option<String>,
    /// The chain the request belongs to, where one is named.
    root: Option<String>,
    /// The span of the stage's own work — `None` for a stage that is not tracing.
    stage_span: Option<String>,
    /// The span of the call OUT to the backend, which [`Self::header`] names as the `Parent`
    /// and the backend's own segment names as its parent.
    call_span: Option<String>,
    /// The span the CALLER named as the parent of this stage's work, where it named one.
    caller_span: Option<String>,
    /// Whether this stage RECORDS the request: it is tracing, the request names a chain, and
    /// the sampling decision says the chain is recorded.
    recorded: bool,
}

impl Propagated {
    /// Whether this stage RECORDS the request, which is what decides both halves above.
    fn records(&self) -> bool {
        self.recorded
    }
}

/// What this stage propagates and records for the call `proxy` describes.
///
/// The chain and the sampling decision are decided ONCE here, for the whole request, and the
/// headers below then name a span per call the stage makes — so a request that reaches both an
/// authorizer's function and the integration's belongs to ONE trace whichever of them the
/// caller's own header left to this stage to decide.
pub(super) fn propagated(proxy: &ProxyInputs, at: SystemTime) -> Propagated {
    let supplied = supplied_trace_header(proxy);
    // The chain: the one the caller named where it named one in X-Ray's own form, one this
    // stage starts where it did not — and, for a stage that records nothing, only the chain
    // the caller named, because such a stage starts none.
    let continued = supplied.and_then(::service_common::xray_trace::continued_root);
    let root = match (proxy.tracing_enabled, continued) {
        (true, Some(continued)) => Some(continued.to_string()),
        (true, None) => Some(::service_common::xray_trace::mint_trace_id(at)),
        (false, continued) => continued.map(str::to_string),
    };
    // The decision: the caller's where it took one, and otherwise this stage's — a stage that
    // records its own work records this request. `Sampled=0` is a request whose work still
    // happens and whose trace is not recorded.
    let sampled = supplied
        .and_then(::service_common::xray_trace::sampling_decision)
        .unwrap_or(proxy.tracing_enabled);
    // A stage that is tracing has a span for its own work and one for its call out to the
    // backend; a stage that is not has neither, and passes the caller's header on as written.
    let (stage_span, call_span) = if proxy.tracing_enabled {
        (
            Some(::service_common::xray_trace::mint_span_id()),
            Some(::service_common::xray_trace::mint_span_id()),
        )
    } else {
        (None, None)
    };
    let stamped = |span: Option<&String>| match (root.as_deref(), span) {
        (Some(root), Some(span)) => Some(::service_common::xray_trace::stamped_tracing_header(
            root, span, sampled,
        )),
        // A stage that records nothing has no span to name and no decision to take, so the
        // caller's own header travels on as written; a request naming no chain carries none.
        _ => supplied
            .and_then(::service_common::xray_trace::tracing_header)
            .map(str::to_string),
    };
    Propagated {
        header: stamped(call_span.as_ref()),
        authorizer_header: stamped(stage_span.as_ref()),
        recorded: sampled && root.is_some() && stage_span.is_some(),
        root,
        stage_span,
        call_span,
        caller_span: supplied
            .and_then(::service_common::xray_trace::parent_span)
            .map(str::to_string),
    }
}

/// The segment document this stage recorded for the request, or `None` where it recorded
/// none: a stage that is not tracing, and a request the decision says is not recorded.
///
/// Every value is read off the call and this api's own records: the chain the header named,
/// the api and stage the request reached, the verb and path it arrived at, the status it was
/// answered with, and the instants the stage began and finished serving it. The call out to
/// the backend is a nested aws-namespace subsegment naming the function the integration
/// addressed, which is the span the function's own segment hangs from.
pub(super) fn stage_segment(
    propagated: &Propagated,
    proxy: &ProxyInputs,
    served: &Served<'_>,
) -> Option<String> {
    if !propagated.records() {
        return None;
    }
    let root = propagated.root.as_deref()?;
    let stage_span = propagated.stage_span.as_deref()?;
    let started = ::service_common::xray_segments::epoch_seconds(served.started);
    let ended = ::service_common::xray_segments::epoch_seconds(served.ended);
    let client_ip = super::source_address(
        proxy
            .multi_value_header
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(super::FORWARDED_FOR_HEADER))
            .and_then(|(_, values)| values.first())
            .map(String::as_str),
    );
    let mut http = json!({
        "request": {
            "method": proxy.http_method,
            "url": requested_url(proxy, served),
            "client_ip": client_ip,
        },
    });
    if let Some(status) = served.status {
        insert(&mut http, "response", json!({ "status": status }));
    }
    let mut document = json!({
        "trace_id": root,
        "id": stage_span,
        "name": format!("{}/{}", served.api_id, served.stage),
        "start_time": started,
        "end_time": ended,
        "origin": STAGE_ORIGIN,
        // The outcome FLAGS X-Ray reads a segment's own result off, derived from the status
        // the request was answered with ([`outcome_flags`]).
        "error": outcome_flags(served.status).error,
        "throttle": outcome_flags(served.status).throttle,
        "fault": outcome_flags(served.status).fault,
        "http": http,
    });
    // The stage's work is a CHILD of the caller's where the caller named its own span, and
    // the root of the trace where it did not — which is what makes a traced stage the one
    // parentless document of a trace it started.
    if let Some(caller_span) = propagated.caller_span.as_deref() {
        insert(&mut document, "parent_id", json!(caller_span));
    }
    if let Some(call_span) = propagated.call_span.as_deref() {
        insert(
            &mut document,
            "subsegments",
            json!([{
                "id": call_span,
                "name": LAMBDA_SUBSEGMENT_NAME,
                "namespace": AWS_NAMESPACE,
                "start_time": started,
                "end_time": ended,
                "aws": { "function_name": reported_function_name(served.function_name) },
            }]),
        );
    }
    Some(document.to_string())
}

/// The three OUTCOME flags a segment document carries, as X-Ray derives each from the status
/// the request was answered with: "`error` — boolean indicating that a client error occurred
/// (response status code was 4XX Client Error)", "`throttle` — boolean indicating that a
/// request was throttled (response status code was 429 Too Many Requests)" and "`fault` —
/// boolean indicating that a server error occurred (response status code was 5XX Server
/// Error)". They are EXCLUSIVE, and a throttle is reported as the throttle rather than as the
/// client error its status also is.
///
/// A request the stage could not serve through its integration at all has no status it was
/// answered with at this layer, and is a fault of the stage's own work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Outcome {
    error: bool,
    throttle: bool,
    fault: bool,
}

/// The flags `status` gives, per the member documentation on [`Outcome`].
fn outcome_flags(status: Option<u16>) -> Outcome {
    let Some(status) = status else {
        return Outcome {
            error: false,
            throttle: false,
            fault: true,
        };
    };
    Outcome {
        error: (400..500).contains(&status) && status != THROTTLED_STATUS,
        throttle: status == THROTTLED_STATUS,
        fault: (500..600).contains(&status),
    }
}

/// The status a THROTTLED request is answered with, which is the one 4xx X-Ray reports as a
/// throttle rather than as a client error.
const THROTTLED_STATUS: u16 = 429;

/// What the stage served, which its own segment reports: the api and stage the request
/// reached, the backend it reached through them, the status it was answered with, and the
/// instants the stage began and finished serving it.
#[derive(Debug)]
pub(super) struct Served<'a> {
    /// `$context.apiId`, the api whose stage served the request.
    pub api_id: &'a str,
    /// `$context.stage`, the stage within it.
    pub stage: &'a str,
    /// The function the integration addressed, as its `uri` named it.
    pub function_name: &'a str,
    /// The status the request was answered with, or `None` for a request the stage could
    /// not serve through its integration at all.
    pub status: Option<u16>,
    /// When the stage began serving it.
    pub started: SystemTime,
    /// When it finished.
    pub ended: SystemTime,
}

/// The URL the request was made to, as a segment's `http.request.url` reports it: X-Ray
/// documents that member as "the full URL of the request, compiled from the protocol,
/// hostname, and path of the request".
///
/// The host is the endpoint the request was ADDRESSED on, which is what the proxy event
/// reports as its `domainName`, and the path is the stage's own prefix and the path within it
/// — the two parts a client's invoke URL is made of. A request that named no host at all has
/// no hostname to compile one from, so the path within the stage is the whole of what is
/// known about it.
fn requested_url(proxy: &ProxyInputs, served: &Served<'_>) -> String {
    let path = format!("/{}{}", served.stage, proxy.request_path);
    if proxy.domain_name.is_empty() {
        return path;
    }
    format!("https://{}{path}", proxy.domain_name)
}

/// The function's own NAME, out of however the integration's `uri` addressed it — which is
/// what `aws.function_name` reports on the subsegment covering the call out.
///
/// `Integration$uri` names the function-invoking action's path, and the function in it may be
/// addressed by its bare name or by its ARN, either of them with a version or alias
/// qualifier after it (`FunctionName` of the Lambda model gives all four forms). The name is
/// what is left after the ARN's `function:` field and before any qualifier.
fn reported_function_name(addressed: &str) -> &str {
    let named = addressed
        .rsplit_once(":function:")
        .map_or(addressed, |(_, rest)| rest);
    named.split(':').next().unwrap_or(named)
}

/// Put `value` on `document`, which is an object every caller here builds with `json!`.
fn insert(document: &mut Value, member: &str, value: Value) {
    if let Some(object) = document.as_object_mut() {
        object.insert(member.to_string(), value);
    }
}

/// The tracing header the CALLER supplied, or `None` where it supplied none.
///
/// Read case-insensitively, because a header name is case-insensitive on the wire and the
/// name a client spells it with is the name the event reports.
fn supplied_trace_header(proxy: &ProxyInputs) -> Option<&str> {
    proxy
        .multi_value_header
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(TRACE_HEADER))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, UNIX_EPOCH};

    use super::{
        outcome_flags, propagated, reported_function_name, requested_url, stage_segment, Outcome,
        ProxyInputs, Served, STAGE_ORIGIN,
    };

    /// The outcome flags a recorded segment carries — over a TABLE whose rows differ along
    /// which CLASS the status the request was answered with falls in, with each published
    /// bound of the two ranges and the one status X-Ray reports as a throttle.
    #[test]
    fn the_recorded_outcome_flags_follow_the_status_the_request_was_answered_with() {
        // (case, the status, (error, throttle, fault))
        let rows = [
            ("a served request", Some(200), (false, false, false)),
            ("a redirect", Some(307), (false, false, false)),
            ("the first client error", Some(400), (true, false, false)),
            ("the last client error", Some(499), (true, false, false)),
            ("a throttled request", Some(429), (false, true, false)),
            ("the first server fault", Some(500), (false, false, true)),
            ("the last server fault", Some(599), (false, false, true)),
            ("an integration the stage could not perform", None, (false, false, true)),
        ];
        for (case, status, (error, throttle, fault)) in rows {
            let actual = outcome_flags(status);
            assert_eq!(
                actual,
                Outcome {
                    error,
                    throttle,
                    fault
                },
                "case {case:?}: expected the flags of {status:?} to be \
                 error={error}/throttle={throttle}/fault={fault}, got {actual:?}"
            );
        }
    }

    /// The URL a recorded segment reports the request was made to — over a TABLE whose rows
    /// differ along whether the request named a host and what path it arrived at within the
    /// stage.
    #[test]
    fn the_recorded_url_is_the_whole_url_the_request_was_made_to() {
        let rows = [
            (
                "an endpoint host and a path",
                "api1234.execute-api.us-east-1.amazonaws.com",
                "/x",
                "https://api1234.execute-api.us-east-1.amazonaws.com/traced/x",
            ),
            (
                "an endpoint host and the stage's own root",
                "api1234.execute-api.us-east-1.amazonaws.com",
                "/",
                "https://api1234.execute-api.us-east-1.amazonaws.com/traced/",
            ),
            (
                "a custom domain and a nested path",
                "api.example.test",
                "/orders/7",
                "https://api.example.test/traced/orders/7",
            ),
            ("a request that named no host", "", "/x", "/traced/x"),
        ];
        for (case, domain_name, request_path, expected) in rows {
            let proxy = ProxyInputs {
                domain_name: domain_name.to_string(),
                request_path: request_path.to_string(),
                ..ProxyInputs::default()
            };
            let actual = requested_url(&proxy, &served());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the url {expected:?}, got {actual:?}"
            );
        }
    }

    /// The name reported for the function an integration addressed — over a TABLE whose rows
    /// differ along WHICH of the four forms `FunctionName` admits the `uri` named it in
    /// (source: `smithy_query(query="shape", shape="NamespacedFunctionName")` of the Lambda
    /// model, whose pattern admits a bare name, an ARN, and either with a qualifier).
    #[test]
    fn the_reported_function_name_is_the_name_however_the_uri_addressed_it() {
        let rows = [
            ("a bare name", "trace-probe", "trace-probe"),
            (
                "a function ARN",
                "arn:aws:lambda:us-east-1:000000000000:function:trace-probe",
                "trace-probe",
            ),
            (
                "a qualified function ARN",
                "arn:aws:lambda:us-east-1:000000000000:function:trace-probe:live",
                "trace-probe",
            ),
            ("a qualified bare name", "trace-probe:7", "trace-probe"),
            (
                "a partial ARN",
                "000000000000:function:trace-probe",
                "trace-probe",
            ),
        ];
        for (case, addressed, expected) in rows {
            let actual = reported_function_name(addressed);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {addressed:?} to name {expected:?}, got {actual:?}"
            );
        }
    }

    /// One `ProxyInputs` naming the header the caller sent and the stage's tracing setting.
    fn call(sent: Option<(&str, &str)>, tracing_enabled: bool) -> ProxyInputs {
        let mut multi_value_header: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Some((name, value)) = sent {
            multi_value_header.insert(name.to_string(), vec![value.to_string()]);
        }
        ProxyInputs {
            http_method: "GET".to_string(),
            request_path: "/x".to_string(),
            multi_value_header,
            tracing_enabled,
            ..ProxyInputs::default()
        }
    }

    /// A stage's propagation of the caller's trace context — over a TABLE whose rows differ
    /// along what the caller supplied (a sampled chain, an unsampled one, a chain with no
    /// decision, a chain with a parent of its own, a malformed root, no header at all), the
    /// SPELLING the caller used, and whether the stage is tracing.
    #[test]
    fn a_stage_continues_the_callers_chain_and_mints_its_own_parent() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let caller_span = "53995c3f42cd8ad8";
        let at = UNIX_EPOCH + Duration::from_secs(0x5759_e988);
        // One row: what the caller sent, what the stage is set to, and what must follow.
        struct Row {
            case: &'static str,
            /// The header name and value the caller sent, or `None` for a caller that sent
            /// none.
            sent: Option<(&'static str, String)>,
            /// `Stage$tracingEnabled`.
            tracing_enabled: bool,
            /// The chain the stage must continue, or `None` where it must start one.
            chain: Option<&'static str>,
            /// The decision the header onward must carry, or `None` for a call that carries
            /// no header onward at all.
            decision: Option<&'static str>,
            /// Whether the stage records a segment of its own.
            records: bool,
        }
        let rows: Vec<Row> = vec![
            Row {
                case: "a sampled chain, on a tracing stage",
                sent: Some(("x-amzn-trace-id", format!("Root={root};Sampled=1"))),
                tracing_enabled: true,
                chain: Some(root),
                decision: Some("1"),
                records: true,
            },
            Row {
                case: "an unsampled chain, on a tracing stage",
                sent: Some(("x-amzn-trace-id", format!("Root={root};Sampled=0"))),
                tracing_enabled: true,
                chain: Some(root),
                decision: Some("0"),
                records: false,
            },
            Row {
                case: "a chain with no decision, on a tracing stage",
                sent: Some(("X-Amzn-Trace-Id", format!("Root={root}"))),
                tracing_enabled: true,
                chain: Some(root),
                decision: Some("1"),
                records: true,
            },
            Row {
                case: "a chain with a parent of its own, on a tracing stage",
                sent: Some((
                    "X-AMZN-TRACE-ID",
                    format!("Root={root};Parent={caller_span};Sampled=1"),
                )),
                tracing_enabled: true,
                chain: Some(root),
                decision: Some("1"),
                records: true,
            },
            Row {
                case: "a root that is not a trace id, on a tracing stage",
                sent: Some((
                    "x-amzn-trace-id",
                    "Root=not-a-trace-id;Sampled=1".to_string(),
                )),
                tracing_enabled: true,
                chain: None,
                decision: Some("1"),
                records: true,
            },
            Row {
                case: "no header at all, on a tracing stage",
                sent: None,
                tracing_enabled: true,
                chain: None,
                decision: Some("1"),
                records: true,
            },
            Row {
                case: "a sampled chain, on a stage that is not tracing",
                sent: Some(("x-amzn-trace-id", format!("Root={root};Sampled=1"))),
                tracing_enabled: false,
                chain: Some(root),
                decision: Some("1"),
                records: false,
            },
            Row {
                case: "no header at all, on a stage that is not tracing",
                sent: None,
                tracing_enabled: false,
                chain: None,
                decision: None,
                records: false,
            },
        ];
        for row in rows {
            let Row {
                case,
                sent,
                tracing_enabled,
                chain,
                decision,
                records,
            } = row;
            let proxy = call(
                sent.as_ref().map(|(name, value)| (*name, value.as_str())),
                tracing_enabled,
            );
            let propagation = propagated(&proxy, at);
            let Some(decision) = decision else {
                assert!(
                    propagation.header.is_none(),
                    "case {case:?}: expected no tracing header onward, got {:?}",
                    propagation.header
                );
                assert!(
                    stage_segment(&propagation, &proxy, &served()).is_none(),
                    "case {case:?}: a stage that names no chain records no segment"
                );
                assert!(
                    propagation.authorizer_header.is_none(),
                    "case {case:?}: expected no tracing header on an authorizer's invocation \
                     either, got {:?}",
                    propagation.authorizer_header
                );
                continue;
            };
            let stamped = propagation
                .header
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: expected a tracing header onward"));
            let stamped_root =
                ::service_common::xray_trace::trace_field(&stamped, "Root").unwrap_or_default();
            assert!(
                ::service_common::xray_trace::is_trace_id(stamped_root),
                "case {case:?}: expected a trace id as the Root of {stamped:?}, got \
                 {stamped_root:?}"
            );
            if let Some(continued) = chain {
                assert_eq!(
                    stamped_root, continued,
                    "case {case:?}: expected {stamped:?} to continue {continued:?}, got \
                     {stamped_root:?}"
                );
            }
            assert_eq!(
                ::service_common::xray_trace::trace_field(&stamped, "Sampled"),
                Some(decision),
                "case {case:?}: expected the decision {decision:?} in {stamped:?}"
            );
            if tracing_enabled {
                let parent = ::service_common::xray_trace::trace_field(&stamped, "Parent")
                    .unwrap_or_default();
                assert!(
                    ::service_common::xray_trace::is_span_id(parent),
                    "case {case:?}: expected a minted span id as the Parent of {stamped:?}, \
                     got {parent:?}"
                );
            } else {
                assert_eq!(
                    Some(stamped.as_str()),
                    sent.as_ref().map(|(_, value)| value.as_str()),
                    "case {case:?}: a stage that records nothing passes the caller's header on \
                     as written"
                );
            }
            // An AUTHORIZER's invocation is the same request's work: the same chain and the
            // same decision as the integration's call, and — on a tracing stage — the stage's
            // OWN span as the parent, rather than the span of the call out to the backend.
            let authorizer = propagation
                .authorizer_header
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: expected a header on the authorizer"));
            for field in ["Root", "Sampled"] {
                assert_eq!(
                    ::service_common::xray_trace::trace_field(&authorizer, field),
                    ::service_common::xray_trace::trace_field(&stamped, field),
                    "case {case:?}: expected the authorizer's invocation in the same chain as \
                     the integration's, got {authorizer:?} beside {stamped:?}"
                );
            }
            if tracing_enabled {
                assert_eq!(
                    ::service_common::xray_trace::trace_field(&authorizer, "Parent"),
                    propagation.stage_span.as_deref(),
                    "case {case:?}: expected the stage's own span as the authorizer's parent, \
                     got {authorizer:?}"
                );
                assert_ne!(
                    ::service_common::xray_trace::trace_field(&authorizer, "Parent"),
                    ::service_common::xray_trace::trace_field(&stamped, "Parent"),
                    "case {case:?}: expected the authorizer's parent to be the stage's own \
                     span rather than its call out, got {authorizer:?}"
                );
            } else {
                assert_eq!(
                    authorizer, stamped,
                    "case {case:?}: a stage that records nothing passes the same header on to \
                     both calls"
                );
            }
            let segment = stage_segment(&propagation, &proxy, &served());
            assert_eq!(
                segment.is_some(),
                records,
                "case {case:?}: expected the stage to record a segment={records}, got \
                 {segment:?}"
            );
            let Some(segment) = segment else { continue };
            let document: ::serde_json::Value =
                ::serde_json::from_str(&segment).expect("the segment document is JSON");
            assert_eq!(
                document["origin"].as_str(),
                Some(STAGE_ORIGIN),
                "case {case:?}: expected the stage's own origin in {segment}"
            );
            assert_eq!(
                document["name"].as_str(),
                Some("api1234/traced"),
                "case {case:?}: expected the api and stage as the name in {segment}"
            );
            assert_eq!(
                document["trace_id"].as_str(),
                Some(stamped_root),
                "case {case:?}: expected the segment filed under the chain the header named, \
                 got {segment}"
            );
            // The segment this stage recorded IS the span the authorizer's invocation names
            // as its parent, so that invocation's own segment hangs from a recorded one.
            assert_eq!(
                document["id"].as_str(),
                ::service_common::xray_trace::trace_field(&authorizer, "Parent"),
                "case {case:?}: expected the authorizer's parent to be the span this segment \
                 records, got {segment}"
            );
            // The call OUT is the span the header named, so the backend's own segment hangs
            // from the very subsegment this one carries.
            let call_out = &document["subsegments"][0];
            assert_eq!(
                call_out["id"].as_str(),
                ::service_common::xray_trace::trace_field(&stamped, "Parent"),
                "case {case:?}: expected the call out to be the span the header named, got \
                 {segment}"
            );
            assert_eq!(
                call_out["namespace"].as_str(),
                Some("aws"),
                "case {case:?}: expected an aws-namespace call out in {segment}"
            );
            assert_eq!(
                call_out["aws"]["function_name"].as_str(),
                Some("probe"),
                "case {case:?}: expected the invoked function named in {segment}"
            );
            let parent_of_stage = document.get("parent_id").and_then(|id| id.as_str());
            let expected_parent = sent
                .as_ref()
                .map(|(_, value)| value.as_str())
                .filter(|value| value.contains("Parent="))
                .map(|_| caller_span);
            assert_eq!(
                parent_of_stage, expected_parent,
                "case {case:?}: expected the stage's parent to be {expected_parent:?}, got \
                 {parent_of_stage:?}"
            );
        }
    }

    /// What the stage served, for the rows above.
    fn served() -> Served<'static> {
        Served {
            api_id: "api1234",
            stage: "traced",
            function_name: "probe",
            status: Some(200),
            started: UNIX_EPOCH + Duration::from_secs(0x5759_e988),
            ended: UNIX_EPOCH + Duration::from_millis(0x5759_e988 * 1000 + 25),
        }
    }
}
