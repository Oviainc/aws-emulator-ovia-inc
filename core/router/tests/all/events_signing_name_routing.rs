// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `events` SigV4 signing name.
//!
//! THREE emulated services resolve to the key `events`: EventBridge
//! (`com.amazonaws.eventbridge#AWSEvents`, sdkId `EventBridge`, `awsJson1_1`,
//! endpointPrefix `events`), the LEGACY CloudWatch Events model of that same service
//! (`com.amazonaws.cloudwatchevents#AWSEvents`, sdkId `CloudWatch Events`, same
//! version, same protocol, same endpointPrefix) and EventBridge V2
//! (`com.amazonaws.eventbridgev2#AWSEventsV2`, `rpcv2Cbor`, endpointPrefix `eventsv2`,
//! signing name `events`). A service that claims on the signing name alone claims the
//! others' traffic too, and the router answers `500 AmbiguousService` for ALL of it —
//! an error code no AWS model declares.
//!
//! The rule these suites pin: a request is claimed by exactly ONE emulated service —
//! the one whose own wire identity matches it, which is its route key (signing name,
//! endpoint prefix, or the sdkId-derived key an alias model answers under) AND the URI
//! path space its protocol publishes; where two mounted models are the SAME AWS service,
//! differing only in a legacy `sdkId`, exactly one of them (the endpoint's current name)
//! claims the shared key and the other answers under its own.
//!
//! Rows therefore span the dimensions that rule generalises over: which protocol the
//! request speaks (`awsJson1_1` at `POST /` vs `rpcv2Cbor` at
//! `POST /service/AWSEventsV2/operation/<Operation>`), which signal resolves the route
//! key (a SigV4 `Authorization` scope, a presigned `X-Amz-Credential`, or the endpoint
//! host on an anonymous request), which key the request names (`events`, `eventsv2`,
//! `cloudwatchevents`), a `Host` that agrees with, disagrees with or says nothing about
//! the target, an operation only the superset model declares, and a path no model binds.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// A SigV4 `Authorization` header whose credential scope names the signing name all
/// three services' models declare. The signature is never checked: the routers here run
/// with request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTEVENTS/20260101/us-east-1/events/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The same header with the scope naming the sdkId-derived key the LEGACY CloudWatch
/// Events model answers under, so the alias crate stays reachable without contending for
/// `events`.
const AUTHORIZATION_LEGACY: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTEVENTS/20260101/us-east-1/cloudwatchevents/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The same credential scope as a presigned URL carries it: in the query string, with
/// its `/` separators percent-encoded.
const PRESIGNED_CREDENTIAL: &str =
    "X-Amz-Credential=AKIDTESTEVENTS%2F20260101%2Fus-east-1%2Fevents%2Faws4_request";

/// How a row names the service that must claim it.
const OWNER_EVENTBRIDGE: &str = "eventbridge (AWSEvents, awsJson1_1)";
const OWNER_EVENTSV2: &str = "eventbridgev2 (AWSEventsV2, rpcv2Cbor)";
const OWNER_CLOUDWATCH: &str = "cloudwatchevents (the legacy AWSEvents alias)";

/// The `rpcv2Cbor` request form of the V2 service: `POST
/// /service/{ServiceShapeName}/operation/{OperationName}`, with this model's service
/// shape `AWSEventsV2`.
const V2_PATH_PREFIX: &str = "/service/AWSEventsV2/operation";

type EventBridge =
    service_com_amazonaws_eventbridge_awsevents::EmulatedService<engine::InMemoryEngine>;
type CloudWatchEvents =
    service_com_amazonaws_cloudwatchevents_awsevents::EmulatedService<engine::InMemoryEngine>;
type EventBridgeV2 =
    service_com_amazonaws_eventbridgev2_awseventsv2::EmulatedService<engine::InMemoryEngine>;

/// The three services that resolve to the `events` key, over one shared engine, exactly
/// as `router/src/lib.rs` mounts them.
fn all_three() -> (EventBridge, CloudWatchEvents, EventBridgeV2) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (
        EventBridge::new(Arc::clone(&engine)),
        CloudWatchEvents::new(Arc::clone(&engine)),
        EventBridgeV2::new(engine),
    )
}

/// A router with all three of them mounted, so a request that two services claim really
/// is refused as `AmbiguousService`.
fn router_with_all_three() -> MultiServiceRouter {
    let (eventbridge, cloudwatch, v2) = all_three();
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(eventbridge);
    router.register_service(cloudwatch);
    router.register_service(v2);
    router
}

/// How a row's request is signed: what resolves its route key.
#[derive(Debug, Clone, Copy)]
enum Signing {
    /// A SigV4 `Authorization` header naming `events`, as every signed SDK request to
    /// this endpoint carries.
    Header,
    /// A SigV4 `Authorization` header naming the legacy alias model's own key.
    LegacyHeader,
    /// A presigned URL's `X-Amz-Credential` query parameter, naming `events`.
    Query,
    /// Nothing at all, so the endpoint host resolves the key.
    Anonymous,
}

/// One row of the claim table: the case name, how the request is signed, its `Host`,
/// its URI, its `X-Amz-Target` if any, and every service that must claim it.
type ClaimRow = (
    &'static str,
    Signing,
    &'static str,
    String,
    Option<&'static str>,
    Vec<&'static str>,
);

/// The request parts the router hands to `Service::can_handle`.
fn parts(signing: Signing, host: &str, uri: &str, target: Option<&str>) -> http::request::Parts {
    let uri = match signing {
        Signing::Query if uri.contains('?') => format!("{uri}&{PRESIGNED_CREDENTIAL}"),
        Signing::Query => format!("{uri}?{PRESIGNED_CREDENTIAL}"),
        _ => uri.to_string(),
    };
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", host);
    match signing {
        Signing::Header => builder = builder.header("authorization", AUTHORIZATION),
        Signing::LegacyHeader => builder = builder.header("authorization", AUTHORIZATION_LEGACY),
        Signing::Query | Signing::Anonymous => {}
    }
    if let Some(target) = target {
        builder = builder.header("x-amz-target", target);
    }
    builder
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// Every service that claims a request, by label — so a row pins BOTH that the right
/// service claims it and that no other does.
fn claimants(
    eventbridge: &EventBridge,
    cloudwatch: &CloudWatchEvents,
    v2: &EventBridgeV2,
    parts: &http::request::Parts,
) -> Vec<&'static str> {
    [
        (OWNER_EVENTBRIDGE, eventbridge.can_handle(parts)),
        (OWNER_CLOUDWATCH, cloudwatch.can_handle(parts)),
        (OWNER_EVENTSV2, v2.can_handle(parts)),
    ]
    .into_iter()
    .filter_map(|(label, claimed)| claimed.then_some(label))
    .collect()
}

/// One answer from the router for a request signed for `events`.
async fn call_json(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String, Value) {
    call_json_signed(router, AUTHORIZATION, operation, body).await
}

/// One answer from the router: the HTTP status, the awsJson `__type` error code (the
/// empty string on a success, which carries no `__type`) and the parsed body.
async fn call_json_signed(
    router: &MultiServiceRouter,
    authorization: &str,
    operation: &str,
    body: &Value,
) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("authorization", authorization)
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    (status, code, parsed)
}

#[test]
fn exactly_one_service_claims_each_request_for_the_events_key() {
    let (eventbridge, cloudwatch, v2) = all_three();

    // (case, how the request is signed, Host, URI, X-Amz-Target, the services that must
    // claim it)
    //
    // The paths come from the two protocols: `awsJson1_1` posts every operation to `/`
    // (neither `events` model binds a `smithy.api#http` trait), and `rpcv2Cbor` posts
    // every operation to `/service/AWSEventsV2/operation/<Operation>`.
    let rows: Vec<ClaimRow> = vec![
        (
            "CreateEventBus, signed, on the events host",
            Signing::Header,
            "events.us-east-1.amazonaws.com",
            "/".to_string(),
            Some("AWSEvents.CreateEventBus"),
            vec![OWNER_EVENTBRIDGE],
        ),
        (
            "CreateEventBus, signed, on a bare emulator endpoint",
            Signing::Header,
            "localhost:9297",
            "/".to_string(),
            Some("AWSEvents.CreateEventBus"),
            vec![OWNER_EVENTBRIDGE],
        ),
        (
            "CreateEventBus, signed, on the V2 endpoint's host: the scope decides",
            Signing::Header,
            "eventsv2.us-east-1.amazonaws.com",
            "/".to_string(),
            Some("AWSEvents.CreateEventBus"),
            vec![OWNER_EVENTBRIDGE],
        ),
        (
            "PutEvents, signed",
            Signing::Header,
            "events.eu-west-2.amazonaws.com",
            "/".to_string(),
            Some("AWSEvents.PutEvents"),
            vec![OWNER_EVENTBRIDGE],
        ),
        (
            "UpdateEventBus, an operation only the superset model declares",
            Signing::Header,
            "events.us-east-1.amazonaws.com",
            "/".to_string(),
            Some("AWSEvents.UpdateEventBus"),
            vec![OWNER_EVENTBRIDGE],
        ),
        (
            "DescribeEventBus, presigned in the query string",
            Signing::Query,
            "events.us-east-1.amazonaws.com",
            "/".to_string(),
            Some("AWSEvents.DescribeEventBus"),
            vec![OWNER_EVENTBRIDGE],
        ),
        (
            "an anonymous request addressed to the events host",
            Signing::Anonymous,
            "events.us-east-1.amazonaws.com",
            "/".to_string(),
            None,
            vec![OWNER_EVENTBRIDGE],
        ),
        (
            "the V2 CreateEventBus, signed for `events`",
            Signing::Header,
            "eventsv2.us-east-1.amazonaws.com",
            format!("{V2_PATH_PREFIX}/CreateEventBus"),
            None,
            vec![OWNER_EVENTSV2],
        ),
        (
            "the V2 ListEventBuses on the events host: the path decides",
            Signing::Header,
            "events.us-east-1.amazonaws.com",
            format!("{V2_PATH_PREFIX}/ListEventBuses"),
            None,
            vec![OWNER_EVENTSV2],
        ),
        (
            "the V2 PutEvents, presigned in the query string",
            Signing::Query,
            "eventsv2.us-east-1.amazonaws.com",
            format!("{V2_PATH_PREFIX}/PutEvents"),
            None,
            vec![OWNER_EVENTSV2],
        ),
        (
            "an anonymous V2 request addressed to the eventsv2 host",
            Signing::Anonymous,
            "eventsv2.us-east-1.amazonaws.com",
            format!("{V2_PATH_PREFIX}/ListEventBuses"),
            None,
            vec![OWNER_EVENTSV2],
        ),
        // The legacy alias model's own key: claimed by that crate alone, so it stays
        // reachable without ever contending for `events`.
        (
            "PutRule signed for the legacy alias model's own key",
            Signing::LegacyHeader,
            "events.us-east-1.amazonaws.com",
            "/".to_string(),
            Some("AWSEvents.PutRule"),
            vec![OWNER_CLOUDWATCH],
        ),
        (
            "an anonymous request addressed to the legacy alias host",
            Signing::Anonymous,
            "cloudwatchevents.us-east-1.amazonaws.com",
            "/".to_string(),
            None,
            vec![OWNER_CLOUDWATCH],
        ),
        (
            "the V2 cbor path space signed for the legacy alias key",
            Signing::LegacyHeader,
            "events.us-east-1.amazonaws.com",
            format!("{V2_PATH_PREFIX}/ListEventBuses"),
            None,
            vec![],
        ),
        // Paths no `events` model binds, and a request for another service entirely:
        // claimed by nobody, which is the router's `404 UnknownService` and NOT an
        // ambiguity.
        (
            "a REST-shaped path no events model binds",
            Signing::Header,
            "events.us-east-1.amazonaws.com",
            "/v1/event-buses".to_string(),
            None,
            vec![],
        ),
        (
            "the cbor path space of some other service shape",
            Signing::Header,
            "events.us-east-1.amazonaws.com",
            "/tags/arn:aws:events:us-east-1:000000000000:event-bus%2Fdefault".to_string(),
            None,
            vec![],
        ),
        (
            "an anonymous request addressed to an unrelated host",
            Signing::Anonymous,
            "sqs.us-east-1.amazonaws.com",
            "/".to_string(),
            None,
            vec![],
        ),
    ];

    for (case, signing, host, uri, target, expected) in rows {
        let parts = parts(signing, host, &uri, target);
        let actual = claimants(&eventbridge, &cloudwatch, &v2, &parts);
        assert_eq!(
            actual, expected,
            "case {case:?} (POST {uri} to host {host}, {signing:?}): expected it to be \
             claimed by exactly {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_events_signed_request_is_served_instead_of_refused_as_ambiguous() {
    let router = router_with_all_three();

    // (case, operation, body, the status the owning service answers or `None` when only
    // "a service answered" is pinned, the error code)
    //
    // Each row names the answer of the ONE service that owns it, which is what excludes
    // both routing failures: `AmbiguousService` (500, several claimants) and
    // `UnknownService` (404, none). `UpdateEventBus` carries `None`: it is modelled only
    // by the superset EventBridge model, so what the row pins is that the claimant KNOWS
    // the operation (the legacy model would answer the awsJson routing 404) without
    // pinning its scaffold 501, which would fail the day it is implemented.
    let rows: [(&str, &str, Value, Option<u16>, &str); 6] = [
        (
            "CreateEventBus on a free name",
            "CreateEventBus",
            json!({ "Name": "sim2real-schemas-bus-x", "Tags": [{ "Key": "sim2real", "Value": "true" }] }),
            Some(200),
            "",
        ),
        (
            "CreateEventBus on the account's default bus",
            "CreateEventBus",
            json!({ "Name": "default" }),
            Some(400),
            "ResourceAlreadyExistsException",
        ),
        (
            "DescribeEventBus with no Name",
            "DescribeEventBus",
            json!({}),
            Some(200),
            "",
        ),
        (
            "DescribeEventBus on a bus that does not exist",
            "DescribeEventBus",
            json!({ "Name": "never-created" }),
            Some(400),
            "ResourceNotFoundException",
        ),
        (
            "PutRule, an EventBridge control-plane operation",
            "PutRule",
            json!({ "Name": "routing-probe", "EventPattern": r#"{"source":["com.demo"]}"# }),
            Some(200),
            "",
        ),
        (
            "UpdateEventBus, declared only by the superset model",
            "UpdateEventBus",
            json!({}),
            None,
            "",
        ),
    ];

    for (case, operation, body, expected_status, expected_code) in rows {
        let (status, code, parsed) = call_json(&router, operation, &body).await;
        assert!(
            code != "AmbiguousService" && code != "UnknownService",
            "case {case:?}: {operation} must be served by exactly one of the three services \
             that resolve to the `events` key, got HTTP {status} {code:?} ({parsed})"
        );
        assert_ne!(
            status, 404,
            "case {case:?}: the claiming service's model must know {operation}, got HTTP \
             {status} {code:?} ({parsed})"
        );
        if let Some(expected_status) = expected_status {
            assert_eq!(
                (status, code.as_str()),
                (expected_status, expected_code),
                "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the \
                 service that owns {operation}, got {status} {code:?} ({parsed})"
            );
        }
    }

    // The same operation signed for the legacy alias model's own key reaches THAT
    // service, which keeps its own rule slice reachable: its `CreateEventBus` is still
    // scaffolded, so the two crates' answers also tell the claimants apart.
    let (status, code, parsed) = call_json_signed(
        &router,
        AUTHORIZATION_LEGACY,
        "PutRule",
        &json!({ "Name": "legacy-probe", "EventPattern": r#"{"source":["com.demo"]}"# }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"PutRule on the legacy alias key\": expected HTTP 200 from the alias service, \
         got {status} {code:?} ({parsed})"
    );
    let (status, code, parsed) = call_json_signed(
        &router,
        AUTHORIZATION_LEGACY,
        "CreateEventBus",
        &json!({ "Name": "legacy-bus" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (501, "NotImplemented"),
        "case \"CreateEventBus on the legacy alias key\": that crate's handler is still \
         scaffolded, so this must come from it rather than from the EventBridge crate, got \
         {status} {code:?} ({parsed})"
    );
}

#[tokio::test]
async fn a_bus_created_through_the_shared_key_reads_back() {
    let router = router_with_all_three();

    let (status, code, created) = call_json(
        &router,
        "CreateEventBus",
        &json!({ "Name": "sim2real-schemas-bus-x" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"CreateEventBus\": expected HTTP 200, got {status} {code:?} ({created})"
    );
    let arn = created["EventBusArn"].as_str().unwrap_or_default();
    assert_eq!(
        arn,
        format!(
            "arn:aws:events:us-east-1:{}:event-bus/sim2real-schemas-bus-x",
            engine::emulator_account_id()
        ),
        "case \"CreateEventBus\": expected the bus ARN of the signed region, got {arn:?}"
    );

    let (status, code, described) = call_json(
        &router,
        "DescribeEventBus",
        &json!({ "Name": arn.to_string() }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"DescribeEventBus after CreateEventBus\": expected HTTP 200, got {status} \
         {code:?} ({described})"
    );
    assert_eq!(
        described["Arn"], created["EventBusArn"],
        "case \"DescribeEventBus after CreateEventBus\": the bus read back must be the bus \
         created, got {described}"
    );
}

#[tokio::test]
async fn an_rpcv2_cbor_request_reaches_the_v2_service() {
    let router = router_with_all_three();

    // A `rpcv2Cbor` request: the empty CBOR map (`0xa0`) as the body, which is a
    // well-formed `ListEventBuses` input (every member of it is optional). Its operation
    // is still scaffolded, so what this pins is that a SERVICE answered — neither of the
    // router's own refusals, which are rendered as JSON whatever the claimant's protocol
    // is.
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{V2_PATH_PREFIX}/ListEventBuses"))
        .header("host", "eventsv2.us-east-1.amazonaws.com")
        .header("content-type", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from_static(&[0xa0]))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let rendered = String::from_utf8_lossy(response.body()).to_string();
    assert!(
        !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
        "a cbor request to the V2 path space must be served by exactly one service, got \
         HTTP {status} with {rendered:?}"
    );
    assert_ne!(
        status, 404,
        "the V2 service's model declares ListEventBuses, got HTTP {status} with {rendered:?}"
    );
    assert_ne!(
        status, 500,
        "a cbor request to the V2 path space must not be refused by the router, got HTTP \
         {status} with {rendered:?}"
    );
}
