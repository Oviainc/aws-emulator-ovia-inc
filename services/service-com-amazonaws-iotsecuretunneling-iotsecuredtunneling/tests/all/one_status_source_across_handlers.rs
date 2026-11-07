// SPDX-License-Identifier: CC0-1.0

//! Every handler that refuses with a modelled error takes its HTTP status from the
//! ONE place that computes it.
//!
//! RULE: the status a modelled error of this service arrives with is computed by
//! `scaffolded::wire_status` alone, so each operation's refusal carries the status
//! that function gives the shape's modelled `@httpError` — never the trait's own
//! value and never a value one handler chose for itself. The modelled `@httpError`
//! of each shape (`ResourceNotFoundException` 404, `LimitExceededException` 403,
//! both `errorKind client`) is from
//! `smithy_query(query="shape", shape="ResourceNotFoundException,LimitExceededException")`.
//!
//! Rows differ along what the rule generalises over: the handler module that
//! raises the refusal (one row at least per module whose prose used to restate a
//! status: `close_tunnel`, `rotate_tunnel_access_token`, `open_tunnel`,
//! `tag_resource`, and the limits `state` keeps), the modelled error shape, the
//! `@httpError` that shape records, how the resource is addressed (`tunnelId` or
//! `resourceArn`), and why the refusal is raised (never created, deleted, a
//! per-resource limit, or a Region quota).
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::scaffolded::wire_status;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::{
    MAX_CONCURRENT_TUNNELS, MAX_TAGS_PER_TUNNEL,
};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the rows are opened and addressed in.
const REGION: &str = "eu-north-1";

/// A second Region, so the per-Region open-tunnel quota can be exhausted without
/// touching the tunnels the other rows read.
const QUOTA_REGION: &str = "ap-south-1";

/// One modelled error shape, with the `@httpError` the model records for it. The
/// trait value is what the wire must NOT use; the status under test is whatever
/// `wire_status` maps it to.
#[derive(Debug, Clone, Copy)]
struct ModeledError {
    code: &'static str,
    modeled_http_error: u16,
}

/// The error every operation that addresses a tunnel declares.
const NOT_FOUND: ModeledError = ModeledError {
    code: "ResourceNotFoundException",
    modeled_http_error: 404,
};

/// The error this service models for an exhausted limit.
const LIMIT_EXCEEDED: ModeledError = ModeledError {
    code: "LimitExceededException",
    modeled_http_error: 403,
};

/// One row: the handler module under test, the request, the endpoint Region it is
/// sent to, and the modelled error it must raise.
#[derive(Debug)]
struct Row {
    case: String,
    module: &'static str,
    region: &'static str,
    operation: &'static str,
    request: Value,
    expected: ModeledError,
}

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

/// Open one tunnel in `region` and return `(tunnelId, tunnelArn)`.
async fn open_tunnel(router: &MultiServiceRouter, region: &str, case: &str) -> (String, String) {
    let (status, opened) = call(
        router,
        region,
        "OpenTunnel",
        &json!({
            "description": case,
            "destinationConfig": {"thingName": "thing-one-status", "services": ["SSH"]},
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
fn tags(count: usize, prefix: &str) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"key": format!("{prefix}{index}"), "value": "v"}))
            .collect(),
    )
}

/// The table: one refusal per handler module whose prose used to carry its own
/// copy of the status, set up over the wire so every row is the service's own
/// answer.
async fn rows(router: &MultiServiceRouter) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // A tunnel this Region's endpoint does hold, so no row is answered out of an
    // empty store.
    open_tunnel(router, REGION, "the live tunnel").await;

    // `close_tunnel`: the tunnel the endpoint holds no record for, by id.
    rows.push(Row {
        case: "CloseTunnel on an id this endpoint holds no record for".to_string(),
        module: "ops/close_tunnel.rs",
        region: REGION,
        operation: "CloseTunnel",
        request: json!({"tunnelId": "no-such-tunnel-1f4c"}),
        expected: NOT_FOUND,
    });

    // `close_tunnel` again, for what `CloseTunnel(delete)` leaves behind: the data
    // is gone, so reading the tunnel back is refused as a missing resource.
    let (deleted_id, _) = open_tunnel(router, REGION, "the deleted tunnel").await;
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
    rows.push(Row {
        case: "DescribeTunnel on a tunnel CloseTunnel(delete) removed".to_string(),
        module: "ops/close_tunnel.rs",
        region: REGION,
        operation: "DescribeTunnel",
        request: json!({"tunnelId": deleted_id}),
        expected: NOT_FOUND,
    });

    // `rotate_tunnel_access_token`: its one modelled error, on the same class.
    rows.push(Row {
        case: "RotateTunnelAccessToken on an unheld tunnel".to_string(),
        module: "ops/rotate_tunnel_access_token.rs",
        region: REGION,
        operation: "RotateTunnelAccessToken",
        request: json!({"tunnelId": "no-such-tunnel-7b03", "clientMode": "ALL"}),
        expected: NOT_FOUND,
    });

    // `open_tunnel` with the per-tunnel tag limit `state` keeps: the other
    // modelled shape, whose `@httpError` is a different value again.
    rows.push(Row {
        case: format!(
            "OpenTunnel with {} distinct tag keys",
            MAX_TAGS_PER_TUNNEL + 1
        ),
        module: "ops/open_tunnel.rs + state.rs",
        region: REGION,
        operation: "OpenTunnel",
        request: json!({
            "destinationConfig": {"thingName": "thing-one-status", "services": ["SSH"]},
            "tags": tags(MAX_TAGS_PER_TUNNEL + 1, "open-key-"),
        }),
        expected: LIMIT_EXCEEDED,
    });

    // `tag_resource`: the same limit, reached by accumulation on a stored tunnel.
    let (_, full_arn) = open_tunnel(router, REGION, "a tunnel at its tag limit").await;
    let (status, tagged) = call(
        router,
        REGION,
        "TagResource",
        &json!({"resourceArn": full_arn, "tags": tags(MAX_TAGS_PER_TUNNEL, "tag-key-")}),
    )
    .await;
    assert_eq!(status, 200, "TagResource answered {status} {tagged:?}");
    rows.push(Row {
        case: "TagResource with one key past the tag limit".to_string(),
        module: "ops/tag_resource.rs + state.rs",
        region: REGION,
        operation: "TagResource",
        request: json!({
            "resourceArn": full_arn,
            "tags": [{"key": "one-too-many", "value": "v"}],
        }),
        expected: LIMIT_EXCEEDED,
    });

    // `state`'s other cap: the tunnels one account holds OPEN in one Region,
    // exhausted in a Region of its own so the rows above are untouched.
    for index in 0..MAX_CONCURRENT_TUNNELS {
        let case = format!("quota filler {index}");
        open_tunnel(router, QUOTA_REGION, &case).await;
    }
    rows.push(Row {
        case: format!("OpenTunnel at the quota of {MAX_CONCURRENT_TUNNELS} open tunnels"),
        module: "ops/open_tunnel.rs + state.rs",
        region: QUOTA_REGION,
        operation: "OpenTunnel",
        request: json!({
            "destinationConfig": {"thingName": "thing-one-status", "services": ["SSH"]},
        }),
        expected: LIMIT_EXCEEDED,
    });

    rows
}

#[tokio::test]
async fn every_handlers_modelled_refusal_takes_the_computed_status() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let table = rows(&router).await;
    let modules: BTreeSet<&str> = table.iter().map(|row| row.module).collect();
    assert!(
        modules.len() >= 4,
        "the table must span every handler module that refuses with a modelled error, got \
         {modules:?}"
    );

    let mut answered: BTreeSet<u16> = BTreeSet::new();
    for row in &table {
        let case = &row.case;
        let module = row.module;
        let expected = wire_status(row.expected.modeled_http_error);
        let (status, body) = call(&router, row.region, row.operation, &row.request).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({module}): expected the status wire_status computes, {expected}, \
             answered {status} {body:?}"
        );
        assert_ne!(
            status, row.expected.modeled_http_error,
            "case {case:?} ({module}): the shape's @httpError is inert on this protocol, \
             answered {status} {body:?}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some(row.expected.code),
            "case {case:?} ({module}): expected the wire code {:?}, answered {status} {body:?}",
            row.expected.code
        );
        assert!(
            !body["message"].as_str().unwrap_or_default().is_empty(),
            "case {case:?} ({module}): the refusal carries a message, answered {status} {body:?}"
        );
        answered.insert(status);
    }

    // One source means one value: no two of these handlers disagree about the
    // status a modelled client error arrives with.
    assert_eq!(
        answered.len(),
        1,
        "every row is a modelled client error, so one status answers them all, got {answered:?}"
    );
}
