// SPDX-License-Identifier: CC0-1.0

//! The HTTP status every MODELLED error of this service arrives with.
//!
//! RULE: this service speaks `awsJson1_1` (`smithy_query(query="service_info")`),
//! an RPC protocol on which the HTTP bindings the model carries are inert — every
//! operation reports `httpBindingsInert: true` — so a modelled CLIENT error is
//! answered with that protocol's client status, 400, and is identified only by the
//! error shape's name in the body's `__type`, whatever `@httpError` the shape
//! records. A client that classifies by status therefore never reads a modelled
//! service exception as a missing route (404) or a denied call (403).
//!
//! Rows differ along what the rule generalises over: which operation carries the
//! request, which modelled error shape it raises, the `@httpError` that shape
//! records (404 and 403, so the status cannot be a copy of the trait), how the
//! resource is addressed (by `tunnelId` or by `resourceArn`), and why the error is
//! raised (never created, deleted, held by another Region, or a cap exhausted).
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::scaffolded::wire_status;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::{
    MAX_CONCURRENT_TUNNELS, MAX_TAGS_PER_TUNNEL,
};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the rows are opened and addressed in.
const REGION: &str = "eu-west-1";

/// A second Region, so a row can address a tunnel through an endpoint that does
/// not hold it.
const OTHER_REGION: &str = "us-west-2";

/// The status the `awsJson1_1` client-error channel uses, which is the status
/// under test.
const CLIENT_ERROR_STATUS: u16 = 400;

/// One modelled error shape of this service, with the `@httpError` the model
/// records for it.
///
/// Both shapes and their traits are from
/// `smithy_query(query="shape", shape="ResourceNotFoundException,LimitExceededException")`:
/// `errorKind client`, `httpError` 404 and 403 respectively. Those are the values
/// the wire must NOT use.
#[derive(Debug, Clone, Copy)]
struct ModeledError {
    code: &'static str,
    inert_http_error: u16,
}

/// The error every operation that addresses a tunnel declares.
const NOT_FOUND: ModeledError = ModeledError {
    code: "ResourceNotFoundException",
    inert_http_error: 404,
};

/// The error `OpenTunnel` declares, and the one this service answers for an
/// exhausted cap.
const LIMIT_EXCEEDED: ModeledError = ModeledError {
    code: "LimitExceededException",
    inert_http_error: 403,
};

/// A router carrying only this service.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation against `region`'s endpoint the way an SDK does, and return
/// `(status, body)`.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    input: &Value,
) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type` on awsJson) of a refusal body.
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or_default()
}

/// Open one tunnel in `region` and return `(tunnelId, tunnelArn)`.
async fn open_tunnel(router: &MultiServiceRouter, region: &str, case: &str) -> (String, String) {
    let (status, opened) = call(
        router,
        region,
        "OpenTunnel",
        &json!({
            "description": case,
            "destinationConfig": {"thingName": "thing-wire-status", "services": ["SSH"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: OpenTunnel answered {status} {opened:?}"
    );
    (
        opened["tunnelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no tunnelId in {opened:?}"))
            .to_string(),
        opened["tunnelArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no tunnelArn in {opened:?}"))
            .to_string(),
    )
}

/// `count` distinct tags, as the `tags` member takes them.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("key-{index}"), "value": "v"}))
            .collect(),
    )
}

/// One row: a request, the endpoint Region it is sent to, and the modelled error
/// it must raise.
#[derive(Debug)]
struct Row {
    case: String,
    region: &'static str,
    operation: &'static str,
    request: Value,
    expected: ModeledError,
}

/// Every call that addresses a tunnel the Region's endpoint does not hold, for an
/// `id` and the `arn` that names it.
fn not_found_calls(id: &str, arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("DescribeTunnel", json!({"tunnelId": id})),
        ("CloseTunnel", json!({"tunnelId": id})),
        ("CloseTunnel", json!({"tunnelId": id, "delete": true})),
        (
            "RotateTunnelAccessToken",
            json!({"tunnelId": id, "clientMode": "ALL"}),
        ),
        (
            "RotateTunnelAccessToken",
            json!({"tunnelId": id, "clientMode": "SOURCE"}),
        ),
        ("ListTagsForResource", json!({"resourceArn": arn})),
        (
            "TagResource",
            json!({"resourceArn": arn, "tags": [{"key": "env", "value": "test"}]}),
        ),
        (
            "UntagResource",
            json!({"resourceArn": arn, "tagKeys": ["env"]}),
        ),
    ]
}

/// The table: every modelled error this service can raise, at every entry point
/// that raises it.
async fn rows(router: &MultiServiceRouter) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // A tunnel this Region's endpoint does hold, so no row below is answered out
    // of an empty store.
    let (_, live_arn) = open_tunnel(router, REGION, "the live tunnel").await;
    let account = live_arn.split(':').nth(4).unwrap_or_default().to_string();

    // Ids satisfying `TunnelId`'s modelled pattern `^[a-zA-Z0-9_\-+=:]{1,128}$`
    // (from `smithy_query(query="shape", shape="TunnelId")`) that name no tunnel:
    // the UUID form AWS mints, a name-like id, one using every character class the
    // pattern admits, the shortest it allows and the longest.
    let mut unknown: Vec<(String, String)> = Vec::new();
    for id in [
        "3c6e0b8a-9c15-4d9e-8b27-0f7a1c2d3e4f".to_string(),
        "no-such-tunnel-9f2b".to_string(),
        "weird_id+with=every:char-class".to_string(),
        "a".to_string(),
        "z".repeat(128),
    ] {
        let arn = format!("arn:aws:iot:{REGION}:{account}:tunnel/{id}");
        unknown.push((id, arn));
    }

    // A tunnel whose data `CloseTunnel(delete)` removed is in the same class as
    // one that never existed.
    let (deleted_id, deleted_arn) = open_tunnel(router, REGION, "the deleted tunnel").await;
    let (status, deleted) = call(
        router,
        REGION,
        "CloseTunnel",
        &json!({"tunnelId": deleted_id, "delete": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CloseTunnel(delete) answered {status} {deleted:?}"
    );
    unknown.push((deleted_id, deleted_arn));

    for (id, arn) in &unknown {
        for (operation, request) in not_found_calls(id, arn) {
            rows.push(Row {
                case: format!("{operation} on the unheld tunnel {id:?}"),
                region: REGION,
                operation,
                request,
                expected: NOT_FOUND,
            });
        }
    }

    // A tunnel another Region holds is not a resource this endpoint holds either:
    // the same modelled error, raised for a different reason.
    let (other_id, other_arn) = open_tunnel(router, OTHER_REGION, "a tunnel of another Region").await;
    for (operation, request) in not_found_calls(&other_id, &other_arn) {
        rows.push(Row {
            case: format!("{operation} on a tunnel held by {OTHER_REGION}"),
            region: REGION,
            operation,
            request,
            expected: NOT_FOUND,
        });
    }

    // The other modelled client error, whose `@httpError` is a different value
    // again: a request past a cap this service enforces.
    rows.push(Row {
        case: format!("OpenTunnel with {} distinct tag keys", MAX_TAGS_PER_TUNNEL + 1),
        region: REGION,
        operation: "OpenTunnel",
        request: json!({
            "destinationConfig": {"thingName": "thing-wire-status", "services": ["SSH"]},
            "tags": tags(MAX_TAGS_PER_TUNNEL + 1),
        }),
        expected: LIMIT_EXCEEDED,
    });

    let (_, full_arn) = open_tunnel(router, REGION, "a tunnel at its tag limit").await;
    let (status, tagged) = call(
        router,
        REGION,
        "TagResource",
        &json!({"resourceArn": full_arn, "tags": tags(MAX_TAGS_PER_TUNNEL)}),
    )
    .await;
    assert_eq!(status, 200, "TagResource answered {status} {tagged:?}");
    rows.push(Row {
        case: "TagResource with one key past the tag limit".to_string(),
        region: REGION,
        operation: "TagResource",
        request: json!({
            "resourceArn": full_arn,
            "tags": [{"key": "one-too-many", "value": "v"}],
        }),
        expected: LIMIT_EXCEEDED,
    });

    rows
}

#[tokio::test]
async fn every_modelled_error_arrives_with_this_protocols_client_status() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let table = rows(&router).await;
    assert!(
        table.len() > not_found_calls("id", "arn").len(),
        "the table must span the service's modelled errors, got {} rows",
        table.len()
    );

    for row in &table {
        let case = &row.case;
        let (status, body) = call(&router, row.region, row.operation, &row.request).await;
        assert_eq!(
            status, CLIENT_ERROR_STATUS,
            "case {case:?}: expected HTTP {CLIENT_ERROR_STATUS}, answered {status} {body:?}"
        );
        assert_ne!(
            status, row.expected.inert_http_error,
            "case {case:?}: the shape's @httpError is inert on this protocol, answered \
             {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            row.expected.code,
            "case {case:?}: expected the wire code {:?}, answered {status} {body:?}",
            row.expected.code
        );
        assert!(
            !body["message"].as_str().unwrap_or_default().is_empty(),
            "case {case:?}: the error carries a message, answered {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn a_served_call_is_untouched_by_the_error_status_mapping() {
    // The control for the table above: the mapping is on the error channel only,
    // so a request the service serves still answers 200 and a request it refuses
    // outside its model still answers the client status with ITS code.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let (tunnel_id, tunnel_arn) = open_tunnel(&router, REGION, "the served tunnel").await;
    for (operation, request) in [
        ("DescribeTunnel", json!({"tunnelId": tunnel_id})),
        ("ListTagsForResource", json!({"resourceArn": tunnel_arn})),
        ("ListTunnels", json!({})),
    ] {
        let (status, body) = call(&router, REGION, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {operation}: a served call answered {status} {body:?}"
        );
        assert!(
            body.get("__type").is_none(),
            "case {operation}: a served call carries no error code, answered {body:?}"
        );
    }

    // A refusal this service's model declares no shape for keeps the code the
    // emulator's own refusal channel uses, at the same client status.
    let (status, refused) = call(
        &router,
        REGION,
        "RotateTunnelAccessToken",
        &json!({"tunnelId": tunnel_id, "clientMode": "SOURCE", "destinationConfig": {"services": ["SSH"]}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (CLIENT_ERROR_STATUS, "ValidationException"),
        "an unmodelled refusal answered {status} {refused:?}"
    );
}

#[tokio::test]
async fn the_region_quota_refusal_takes_the_same_status() {
    // The cap that is a quota rather than a per-resource limit, so the modelled
    // limit error is covered at both of its entry points. Filling the quota over
    // the wire is what makes this the service's own answer and not the store's.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for index in 0..MAX_CONCURRENT_TUNNELS {
        let case = format!("quota filler {index}");
        open_tunnel(&router, REGION, &case).await;
    }

    let (status, refused) = call(
        &router,
        REGION,
        "OpenTunnel",
        &json!({"destinationConfig": {"thingName": "thing-wire-status", "services": ["SSH"]}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused)),
        (CLIENT_ERROR_STATUS, LIMIT_EXCEEDED.code),
        "an account at its open-tunnel quota answered {status} {refused:?}"
    );
    assert_ne!(
        status, LIMIT_EXCEEDED.inert_http_error,
        "the shape's @httpError is inert on this protocol, answered {status} {refused:?}"
    );
}

#[test]
fn the_mapping_replaces_every_client_status_and_no_server_one() {
    // The rule as an invariant over the whole status space the mapping can see:
    // a 4xx in the model becomes this protocol's client status, and a 5xx — what
    // an SDK retry classifier must still see — is left exactly as modelled.
    for modeled in 100u16..600 {
        let expected = if (400..500).contains(&modeled) {
            CLIENT_ERROR_STATUS
        } else {
            modeled
        };
        assert_eq!(
            wire_status(modeled),
            expected,
            "case modelled status {modeled}: expected {expected}, got {}",
            wire_status(modeled)
        );
    }
}
