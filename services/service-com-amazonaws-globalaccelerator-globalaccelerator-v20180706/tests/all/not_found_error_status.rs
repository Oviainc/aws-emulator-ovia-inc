// SPDX-License-Identifier: CC0-1.0

//! The status and the wording this service answers a reference to a resource it does
//! not hold with, over every operation that can answer one.
//!
//! RULE under test: a request that references a Global Accelerator resource the store
//! does not hold is refused with the operation's own modelled not-found error, and that
//! error is serialized at HTTP **400** — the status this service's frontend answers a
//! missing resource with — rather than at the 404 the not-found shapes' `@httpError`
//! declares. It is a property of the error class, not of one operation: every modelled
//! error that is NOT a not-found keeps the status its own `@httpError` declares, which
//! the second test below pins.
//!
//! The error catalog the rows are drawn from is the model's own
//! (`smithy_query(query="shape", shape="AcceleratorNotFoundException,
//! ListenerNotFoundException,EndpointGroupNotFoundException,
//! AcceleratorNotDisabledException,InvalidPortRangeException,AccessDeniedException")`):
//! the three not-found shapes are `errorKind` client with `@httpError 404`,
//! `AcceleratorNotDisabledException` and `InvalidPortRangeException` carry
//! `@httpError 400`, `AccessDeniedException` carries `@httpError 403`.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: GlobalAccelerator_V20180706.<Operation>`, `awsJson1_1` body), so the
//! routing table, the handler and the error envelope are all exercised.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A fresh emulator: one shared engine, this service mounted, signature checking off
/// (these tests are about the service's behaviour, not SigV4).
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_globalaccelerator_globalaccelerator_v20180706::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one operation and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "globalaccelerator.us-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("GlobalAccelerator_V20180706.{operation}"),
        )
        // The routing key comes from the SigV4 credential scope's service field.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-west-2/globalaccelerator/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Call one operation and require a 200.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code out of an awsJson error envelope's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The error message out of an awsJson error envelope. A modelled error renders its
/// member as the model spells it (`Message`); the boundary's own envelope uses
/// `message`, so both spellings are read here.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One accelerator, one listener on it and one endpoint group on that listener, each
/// ARN read back out of the create's own answer.
struct Fixture {
    accelerator_arn: String,
    listener_arn: String,
    endpoint_group_arn: String,
}

/// Build the three-level hierarchy this suite reads, refuses and deletes.
async fn fixture(router: &MultiServiceRouter, label: &str) -> Fixture {
    let accelerator_arn = ok(
        router,
        "CreateAccelerator",
        json!({ "Name": format!("nf-{label}"), "IdempotencyToken": format!("{label}-acc") }),
    )
    .await["Accelerator"]["AcceleratorArn"]
        .as_str()
        .expect("the create answers an AcceleratorArn")
        .to_string();
    let listener_arn = ok(
        router,
        "CreateListener",
        json!({
            "AcceleratorArn": accelerator_arn,
            "PortRanges": [{ "FromPort": 80, "ToPort": 81 }],
            "Protocol": "TCP",
            "IdempotencyToken": format!("{label}-lis"),
        }),
    )
    .await["Listener"]["ListenerArn"]
        .as_str()
        .expect("the create answers a ListenerArn")
        .to_string();
    let endpoint_group_arn = ok(
        router,
        "CreateEndpointGroup",
        json!({
            "ListenerArn": listener_arn,
            "EndpointGroupRegion": "eu-west-1",
            "IdempotencyToken": format!("{label}-egp"),
        }),
    )
    .await["EndpointGroup"]["EndpointGroupArn"]
        .as_str()
        .expect("the create answers an EndpointGroupArn")
        .to_string();

    Fixture {
        accelerator_arn,
        listener_arn,
        endpoint_group_arn,
    }
}

/// RULE: a reference to a resource the store does not hold is the operation's own
/// modelled not-found error at HTTP 400, whatever the resource level (accelerator,
/// listener, endpoint group), whatever the operation (read, delete, update, create of a
/// child), and whether the ARN was never minted or named a resource since deleted.
#[tokio::test]
async fn a_missing_resource_is_the_modelled_not_found_error_at_status_400() {
    let router = emulator();

    // Row source 1: ARNs of the right shape that were never minted.
    let unminted_accelerator =
        "arn:aws:globalaccelerator::000000000000:accelerator/00000000-0000-4000-8000-000000000000";
    let unminted_listener = format!("{unminted_accelerator}/listener/deadbeef");
    let unminted_group = format!("{unminted_listener}/endpoint-group/feedface");

    // Row source 2: a hierarchy that really existed and was then deleted, bottom up —
    // the same rule has to hold for an ARN the emulator itself minted.
    let deleted = fixture(&router, "deleted").await;
    ok(
        &router,
        "DeleteEndpointGroup",
        json!({ "EndpointGroupArn": deleted.endpoint_group_arn }),
    )
    .await;
    ok(
        &router,
        "DeleteListener",
        json!({ "ListenerArn": deleted.listener_arn }),
    )
    .await;
    ok(
        &router,
        "UpdateAccelerator",
        json!({ "AcceleratorArn": deleted.accelerator_arn, "Enabled": false }),
    )
    .await;
    ok(
        &router,
        "DeleteAccelerator",
        json!({ "AcceleratorArn": deleted.accelerator_arn }),
    )
    .await;

    // (case, operation, body, expected error code)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        // The accelerator level: the ARN the request carried names nothing.
        (
            "unminted/DescribeAccelerator",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": unminted_accelerator }),
            "AcceleratorNotFoundException",
        ),
        (
            "deleted/DescribeAccelerator",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": deleted.accelerator_arn }),
            "AcceleratorNotFoundException",
        ),
        (
            "unminted/DeleteAccelerator",
            "DeleteAccelerator",
            json!({ "AcceleratorArn": unminted_accelerator }),
            "AcceleratorNotFoundException",
        ),
        (
            "deleted/DeleteAccelerator",
            "DeleteAccelerator",
            json!({ "AcceleratorArn": deleted.accelerator_arn }),
            "AcceleratorNotFoundException",
        ),
        (
            "unminted/UpdateAccelerator",
            "UpdateAccelerator",
            json!({ "AcceleratorArn": unminted_accelerator, "Enabled": true }),
            "AcceleratorNotFoundException",
        ),
        (
            "unminted/ListListeners",
            "ListListeners",
            json!({ "AcceleratorArn": unminted_accelerator }),
            "AcceleratorNotFoundException",
        ),
        (
            "deleted/ListListeners",
            "ListListeners",
            json!({ "AcceleratorArn": deleted.accelerator_arn }),
            "AcceleratorNotFoundException",
        ),
        // A create whose parent accelerator does not exist.
        (
            "unminted/CreateListener",
            "CreateListener",
            json!({
                "AcceleratorArn": unminted_accelerator,
                "PortRanges": [{ "FromPort": 443, "ToPort": 443 }],
                "Protocol": "TCP",
                "IdempotencyToken": "unminted-parent",
            }),
            "AcceleratorNotFoundException",
        ),
        (
            "deleted/CreateListener",
            "CreateListener",
            json!({
                "AcceleratorArn": deleted.accelerator_arn,
                "PortRanges": [{ "FromPort": 8080, "ToPort": 8081 }],
                "Protocol": "UDP",
                "IdempotencyToken": "deleted-parent",
            }),
            "AcceleratorNotFoundException",
        ),
        // The listener level.
        (
            "unminted/DescribeListener",
            "DescribeListener",
            json!({ "ListenerArn": unminted_listener }),
            "ListenerNotFoundException",
        ),
        (
            "deleted/DescribeListener",
            "DescribeListener",
            json!({ "ListenerArn": deleted.listener_arn }),
            "ListenerNotFoundException",
        ),
        (
            "unminted/DeleteListener",
            "DeleteListener",
            json!({ "ListenerArn": unminted_listener }),
            "ListenerNotFoundException",
        ),
        (
            "unminted/UpdateListener",
            "UpdateListener",
            json!({ "ListenerArn": unminted_listener, "Protocol": "UDP" }),
            "ListenerNotFoundException",
        ),
        (
            "deleted/UpdateListener",
            "UpdateListener",
            json!({ "ListenerArn": deleted.listener_arn, "ClientAffinity": "SOURCE_IP" }),
            "ListenerNotFoundException",
        ),
        (
            "unminted/CreateEndpointGroup",
            "CreateEndpointGroup",
            json!({
                "ListenerArn": unminted_listener,
                "EndpointGroupRegion": "us-east-2",
                "IdempotencyToken": "unminted-listener",
            }),
            "ListenerNotFoundException",
        ),
        // The endpoint-group level.
        (
            "unminted/DescribeEndpointGroup",
            "DescribeEndpointGroup",
            json!({ "EndpointGroupArn": unminted_group }),
            "EndpointGroupNotFoundException",
        ),
        (
            "deleted/DescribeEndpointGroup",
            "DescribeEndpointGroup",
            json!({ "EndpointGroupArn": deleted.endpoint_group_arn }),
            "EndpointGroupNotFoundException",
        ),
        (
            "unminted/DeleteEndpointGroup",
            "DeleteEndpointGroup",
            json!({ "EndpointGroupArn": unminted_group }),
            "EndpointGroupNotFoundException",
        ),
        (
            "deleted/DeleteEndpointGroup",
            "DeleteEndpointGroup",
            json!({ "EndpointGroupArn": deleted.endpoint_group_arn }),
            "EndpointGroupNotFoundException",
        ),
    ];

    for (case, operation, body, expected_code) in rows {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, expected_code),
            "case {case:?}: expected HTTP 400 {expected_code}, got {status} with {parsed}"
        );
        // The not-found body still carries a populated Message, as the shape declares.
        assert!(
            !error_message(&parsed).is_empty(),
            "case {case:?}: expected a populated Message, got {parsed}"
        );
    }
}

/// RULE (the other half): the status is a property of the ERROR CLASS, so a modelled
/// error that is not a not-found keeps the status its own `@httpError` declares — 400
/// for the state and port-range refusals, 403 for the authorization refusal.
#[tokio::test]
async fn a_modelled_error_that_is_not_a_not_found_keeps_its_declared_status() {
    let router = emulator();
    let live = fixture(&router, "live").await;

    // (case, operation, body, expected status, expected error code)
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "enabled-accelerator-cannot-be-deleted",
            "DeleteAccelerator",
            json!({ "AcceleratorArn": live.accelerator_arn }),
            400,
            "AcceleratorNotDisabledException",
        ),
        (
            "port-range-upside-down",
            "CreateListener",
            json!({
                "AcceleratorArn": live.accelerator_arn,
                "PortRanges": [{ "FromPort": 8080, "ToPort": 80 }],
                "Protocol": "TCP",
                "IdempotencyToken": "upside-down",
            }),
            400,
            "InvalidPortRangeException",
        ),
        (
            "endpoint-claims-an-attachment-that-grants-nothing",
            "CreateEndpointGroup",
            json!({
                "ListenerArn": live.listener_arn,
                "EndpointGroupRegion": "us-east-1",
                "IdempotencyToken": "attachment",
                "EndpointConfigurations": [{
                    "EndpointId": "i-00000000000000003",
                    "AttachmentArn":
                        "arn:aws:globalaccelerator::000000000000:attachment/00000000",
                }],
            }),
            403,
            "AccessDeniedException",
        ),
    ];

    for (case, operation, body, expected_status, expected_code) in rows {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, \
             got {status} with {parsed}"
        );
    }

    // The endpoint group the fixture created is still there: none of the refusals above
    // changed the store.
    let read = ok(
        &router,
        "DescribeEndpointGroup",
        json!({ "EndpointGroupArn": live.endpoint_group_arn }),
    )
    .await;
    assert_eq!(
        read["EndpointGroup"]["EndpointGroupArn"],
        json!(live.endpoint_group_arn),
        "case re-read: expected the fixture's endpoint group to survive, got {read}"
    );
}

/// RULE: the message of a not-found refusal is the wording this service states that
/// error class with — its fixed text, plus the resource identifier from the request
/// where the wording carries one: a read, delete or update that was ASKED about one
/// named resource names it; a create's parent-accelerator check states a sentence with
/// no variable part.
#[tokio::test]
async fn a_not_found_message_is_the_wording_for_that_error_class() {
    let router = emulator();

    // Two ARNs per level, so each message is tested as a function of the request rather
    // than on one value.
    let first =
        "arn:aws:globalaccelerator::000000000000:accelerator/11111111-1111-4111-8111-111111111111";
    let second =
        "arn:aws:globalaccelerator::000000000000:accelerator/22222222-2222-4222-8222-222222222222";

    // (case, operation, body, expected message)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "first/DescribeAccelerator",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": first }),
            format!("Accelerator {first} not found."),
        ),
        (
            "second/DescribeAccelerator",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": second }),
            format!("Accelerator {second} not found."),
        ),
        (
            "first/DescribeListener",
            "DescribeListener",
            json!({ "ListenerArn": format!("{first}/listener/0a0a0a0a") }),
            format!("Listener {first}/listener/0a0a0a0a not found."),
        ),
        (
            "second/DescribeListener",
            "DescribeListener",
            json!({ "ListenerArn": format!("{second}/listener/0b0b0b0b") }),
            format!("Listener {second}/listener/0b0b0b0b not found."),
        ),
        (
            "first/DescribeEndpointGroup",
            "DescribeEndpointGroup",
            json!({ "EndpointGroupArn":
                format!("{first}/listener/0a0a0a0a/endpoint-group/1a1a1a1a") }),
            format!(
                "EndpointGroup {first}/listener/0a0a0a0a/endpoint-group/1a1a1a1a does not exist."
            ),
        ),
        (
            "second/DeleteEndpointGroup",
            "DeleteEndpointGroup",
            json!({ "EndpointGroupArn":
                format!("{second}/listener/0b0b0b0b/endpoint-group/2b2b2b2b") }),
            format!(
                "EndpointGroup {second}/listener/0b0b0b0b/endpoint-group/2b2b2b2b does not exist."
            ),
        ),
        // The create path's parent lookup: one sentence, no variable part, so two
        // different parent ARNs get the same text.
        (
            "first/CreateListener",
            "CreateListener",
            json!({
                "AcceleratorArn": first,
                "PortRanges": [{ "FromPort": 53, "ToPort": 53 }],
                "Protocol": "UDP",
                "IdempotencyToken": "first-parent",
            }),
            "Could not find accelerator.".to_string(),
        ),
        (
            "second/CreateListener",
            "CreateListener",
            json!({
                "AcceleratorArn": second,
                "PortRanges": [{ "FromPort": 53, "ToPort": 53 }],
                "Protocol": "UDP",
                "IdempotencyToken": "second-parent",
            }),
            "Could not find accelerator.".to_string(),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}
