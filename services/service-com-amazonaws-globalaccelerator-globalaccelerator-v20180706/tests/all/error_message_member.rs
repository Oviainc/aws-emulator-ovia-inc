// SPDX-License-Identifier: CC0-1.0

//! Where this service reports the text of an error, driven over the wire.
//!
//! RULE under test: this service speaks `awsJson1_1`, where a structure member's wire name
//! IS its Smithy member name, and every error shape its model declares carries its text in
//! a member spelled `Message` (and nothing else) — so EVERY error body it answers reports
//! that text under `Message`, whatever produced the error: a handler returning a modelled
//! exception, the boundary validator refusing a value outside a member's declared bound, or
//! the scaffold answering for an operation nobody has written yet. A body that reports the
//! text under the framework's own `message` instead carries no member any client of this
//! service reads, so the failure surfaces with no text at all.
//!
//! The rows below span that set rather than one request: every constraint class the model
//! declares (`@range`, `@length`, `@pattern`, an enum value set, `@required`) on both
//! top-level and nested members, several operations (implemented and still scaffolded), and
//! the modelled exceptions a handler raises itself. They also pin WHICH text each class
//! carries: the per-member constraint report for a member of the request, and the fixed
//! `Invalid parameter.` this service states a tag it rejects with. The error catalog is the
//! model's own
//! (`smithy_query(query="shape",shape="InvalidArgumentException,AcceleratorNotFoundException,AcceleratorNotDisabledException,ListenerNotFoundException,EndpointGroupNotFoundException")`:
//! each is a structure whose single member is `Message`, target `ErrorMessage`, with no
//! `@jsonName`).
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: GlobalAccelerator_V20180706.<Operation>`, `awsJson1_1` body), so the
//! boundary validator, the routing table, the handler and the error envelope are all
//! exercised — not just a helper function.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The member this service's model declares an error's text in.
const MODELLED_MESSAGE_MEMBER: &str = "Message";

/// The member the framework's own error renderers write that text in.
const RENDERED_MESSAGE_MEMBER: &str = "message";

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

/// Assert the one rule over one answered call: the body reports its text in the member the
/// model declares, under that member ALONE, beside the error code it was answered with.
fn assert_text_in_the_modelled_member(case: &str, status: u16, body: &Value) {
    assert!(
        status >= 400,
        "case {case:?}: the row must be a failure to say anything about an error body, \
         got {status} {body}"
    );
    let text = body
        .get(MODELLED_MESSAGE_MEMBER)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the error body must carry a {MODELLED_MESSAGE_MEMBER} member, \
                 got {status} {body}"
            )
        });
    assert!(
        !text.trim().is_empty(),
        "case {case:?}: {MODELLED_MESSAGE_MEMBER} must say what was wrong, got {text:?}"
    );
    assert!(
        body.get(RENDERED_MESSAGE_MEMBER).is_none(),
        "case {case:?}: the text must be reported under {MODELLED_MESSAGE_MEMBER} only, but \
         the body also carries {RENDERED_MESSAGE_MEMBER}: {body}"
    );
    assert!(
        !error_code(body).is_empty(),
        "case {case:?}: the error body must still name its error class, got {body}"
    );
}

/// What the refusal of one request must say, beside carrying its text in the modelled
/// member.
#[derive(Debug)]
enum Expect<'a> {
    /// The per-member constraint report: the envelope over a clause naming the member's
    /// path and the requirement it failed.
    Report {
        /// The member path the clause names (the validator's lowerCamel path).
        path: &'a str,
        /// A fragment of the requirement the clause states.
        requirement: &'a str,
    },
    /// The fixed sentence this service states a tag it rejects with, which names no member
    /// and echoes no value.
    TagSentence,
}

/// The sentence this service states a rejected tag with.
const INVALID_PARAMETER: &str = "Invalid parameter.";

/// RULE: every error body the BOUNDARY renders — a refusal of a value outside a constraint
/// the model declares, before any handler runs — reports its text in this service's
/// `Message` member, and that text is the one this service states for the class: the
/// per-member constraint report for a member of the request, and the fixed
/// `Invalid parameter.` for a tag the request carried.
#[tokio::test]
async fn a_boundary_refusal_reports_its_text_in_the_modelled_message_member() {
    let router = emulator();
    let accelerator_arn = ok(
        &router,
        "CreateAccelerator",
        json!({ "Name": "message-member", "IdempotencyToken": "message-member-token" }),
    )
    .await["Accelerator"]["AcceleratorArn"]
        .as_str()
        .expect("the created accelerator's ARN")
        .to_string();
    let listener_arn = format!("{accelerator_arn}/listener/00000000");
    let endpoint_group_arn = format!("{listener_arn}/endpoint-group/00000000");
    // One past the 255-character bound `GenericString` declares.
    let overlong_arn = "a".repeat(256);

    // (case, operation, body, what the refusal must say) — one row per constraint class the
    // model declares, on a top-level member and on a member nested inside a list of
    // structures, across implemented and still-scaffolded operations. Each is refused
    // BEFORE its handler, so the body under test is the framework's own.
    let cases: Vec<(&str, &str, Value, Expect)> = vec![
        (
            "a float member past its range maximum, on an update",
            "UpdateEndpointGroup",
            json!({ "EndpointGroupArn": endpoint_group_arn, "TrafficDialPercentage": 150.0 }),
            Expect::Report {
                path: "trafficDialPercentage",
                requirement: "Member must have value less than or equal to 100",
            },
        ),
        (
            "a float member below its range minimum, on a create",
            "CreateEndpointGroup",
            json!({
                "ListenerArn": listener_arn,
                "EndpointGroupRegion": "us-west-2",
                "IdempotencyToken": "below-the-dial",
                "TrafficDialPercentage": -1.0,
            }),
            Expect::Report {
                path: "trafficDialPercentage",
                requirement: "Member must have value greater than or equal to 0",
            },
        ),
        (
            "an integer member past its range maximum",
            "CreateEndpointGroup",
            json!({
                "ListenerArn": listener_arn,
                "EndpointGroupRegion": "us-west-2",
                "IdempotencyToken": "past-the-interval",
                "HealthCheckIntervalSeconds": 31,
            }),
            Expect::Report {
                path: "healthCheckIntervalSeconds",
                requirement: "Member must have value less than or equal to 30",
            },
        ),
        (
            "an integer member below its range minimum, on an implemented operation",
            "ListAccelerators",
            json!({ "MaxResults": 0 }),
            Expect::Report {
                path: "maxResults",
                requirement: "Member must have value greater than or equal to 1",
            },
        ),
        (
            "a string member past its length maximum",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": overlong_arn }),
            Expect::Report {
                path: "acceleratorArn",
                requirement: "Member must have length less than or equal to 255",
            },
        ),
        (
            "a list member below its length minimum",
            "CreateListener",
            json!({
                "AcceleratorArn": accelerator_arn,
                "PortRanges": [],
                "Protocol": "TCP",
                "IdempotencyToken": "empty-ranges",
            }),
            Expect::Report {
                path: "portRanges",
                requirement: "Member must have length greater than or equal to 1",
            },
        ),
        (
            "a value outside an enumerated member's value set",
            "CreateEndpointGroup",
            json!({
                "ListenerArn": listener_arn,
                "EndpointGroupRegion": "us-west-2",
                "IdempotencyToken": "unknown-protocol",
                "HealthCheckProtocol": "SMTP",
            }),
            Expect::Report {
                path: "healthCheckProtocol",
                requirement: "enum value set",
            },
        ),
        (
            "a value outside a pattern member's expression",
            "CreateEndpointGroup",
            json!({
                "ListenerArn": listener_arn,
                "EndpointGroupRegion": "us-west-2",
                "IdempotencyToken": "pathless",
                "HealthCheckPath": "health",
            }),
            Expect::Report {
                path: "healthCheckPath",
                requirement: "regular expression pattern",
            },
        ),
        (
            "a required member the request omitted",
            "CreateAccelerator",
            json!({ "IdempotencyToken": "no-name" }),
            Expect::Report {
                path: "name",
                requirement: "Member must not be null",
            },
        ),
        // Tags: the class this service states in its own words, over both bounds the model
        // declares for a tag, on each operation that carries one.
        (
            "a tag key below its length minimum",
            "TagResource",
            json!({
                "ResourceArn": accelerator_arn,
                "Tags": [{ "Key": "", "Value": "empty-key" }],
            }),
            Expect::TagSentence,
        ),
        (
            "a tag value past its length maximum",
            "TagResource",
            json!({
                "ResourceArn": accelerator_arn,
                "Tags": [{ "Key": "k", "Value": "v".repeat(257) }],
            }),
            Expect::TagSentence,
        ),
        (
            "a tag key past its length maximum, on a tag beyond the first",
            "TagResource",
            json!({
                "ResourceArn": accelerator_arn,
                "Tags": [
                    { "Key": "fine", "Value": "fine" },
                    { "Key": "k".repeat(129), "Value": "overlong-key" },
                ],
            }),
            Expect::TagSentence,
        ),
        (
            "a tag carried by a create rather than by the tagging surface",
            "CreateAccelerator",
            json!({
                "Name": "tagged-create",
                "IdempotencyToken": "tagged-create-token",
                "Tags": [{ "Key": "", "Value": "empty-key" }],
            }),
            Expect::TagSentence,
        ),
        (
            // A list of bare tag KEYS is a different shape from a tag, and nothing records
            // how this service states a refusal of one, so it keeps the per-member report.
            "a tag key the untagging surface carries",
            "UntagResource",
            json!({ "ResourceArn": accelerator_arn, "TagKeys": [""] }),
            Expect::Report {
                path: "tagKeys.1.member",
                requirement: "Member must have length greater than or equal to 1",
            },
        ),
        (
            // A tag bound AND a member of the request's own: the per-member report is what
            // names both, so the tag sentence does not take over.
            "a tag key and a member of the request both outside their bounds",
            "CreateAccelerator",
            json!({
                "IdempotencyToken": "tagged-and-nameless",
                "Tags": [{ "Key": "", "Value": "empty-key" }],
            }),
            Expect::Report {
                path: "name",
                requirement: "Member must not be null",
            },
        ),
    ];

    for (case, operation, body, expectation) in cases {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "InvalidArgumentException"),
            "case {case:?}: expected this service's modelled client-input refusal, \
             got {status} {parsed}"
        );
        assert_text_in_the_modelled_member(case, status, &parsed);
        let text = parsed[MODELLED_MESSAGE_MEMBER].as_str().unwrap_or_default();
        match expectation {
            Expect::Report { path, requirement } => {
                assert!(
                    text.starts_with("1 validation error detected: "),
                    "case {case:?}: expected the per-member constraint report, got {text:?}"
                );
                assert!(
                    text.contains(&format!("at '{path}'")),
                    "case {case:?}: expected the clause to name the member {path:?}, \
                     got {text:?}"
                );
                assert!(
                    text.contains(requirement),
                    "case {case:?}: expected the clause to state {requirement:?}, got {text:?}"
                );
            }
            Expect::TagSentence => assert_eq!(
                text, INVALID_PARAMETER,
                "case {case:?}: expected {INVALID_PARAMETER:?}, got {text:?}"
            ),
        }
    }
}

/// RULE: the same holds for an error body a HANDLER produces — the modelled exception it
/// returns is serialized from the model's own shape, so its text is in `Message` and in no
/// other member — and for the scaffold's own answer for an operation nobody has written
/// yet, which the framework renders.
#[tokio::test]
async fn a_handler_error_and_the_scaffold_report_their_text_in_the_modelled_message_member() {
    let router = emulator();
    let enabled_arn = ok(
        &router,
        "CreateAccelerator",
        json!({ "Name": "handler-errors", "IdempotencyToken": "handler-errors-token", "Enabled": true }),
    )
    .await["Accelerator"]["AcceleratorArn"]
        .as_str()
        .expect("the created accelerator's ARN")
        .to_string();
    let missing_accelerator =
        "arn:aws:globalaccelerator::000000000000:accelerator/00000000-0000-4000-8000-000000000000";
    let missing_listener = format!("{missing_accelerator}/listener/deadbeef");

    // (case, operation, body, expected status, expected error code)
    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "an accelerator the store does not hold",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": missing_accelerator }),
            400,
            "AcceleratorNotFoundException",
        ),
        (
            "a listener the store does not hold",
            "DescribeListener",
            json!({ "ListenerArn": missing_listener }),
            400,
            "ListenerNotFoundException",
        ),
        (
            "an accelerator that is still enabled",
            "DeleteAccelerator",
            json!({ "AcceleratorArn": enabled_arn }),
            400,
            "AcceleratorNotDisabledException",
        ),
        (
            "the listeners of an accelerator the store does not hold",
            "ListListeners",
            json!({ "AcceleratorArn": missing_accelerator }),
            400,
            "AcceleratorNotFoundException",
        ),
        (
            // Still scaffolded: the emulator saying nobody has written the operation yet,
            // which is also an error body a client reads the text of.
            "an operation nobody has written yet",
            "ListTagsForResource",
            json!({ "ResourceArn": missing_accelerator }),
            501,
            "NotImplemented",
        ),
        (
            "a second scaffolded operation, over a valid input",
            "TagResource",
            json!({
                "ResourceArn": missing_accelerator,
                "Tags": [{ "Key": "owner", "Value": "team" }],
            }),
            501,
            "NotImplemented",
        ),
    ];

    for (case, operation, body, expected_status, expected_code) in cases {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {parsed}"
        );
        assert_text_in_the_modelled_member(case, status, &parsed);
    }
}

/// RULE: only ERROR bodies are reported this way. A success is answered byte for byte as
/// its codec serialized it — a member of a successful response that happens to be called
/// `message` is a modelled member, not an error's text — and the resource it reports still
/// reads back through its own describe.
#[tokio::test]
async fn a_success_is_untouched_and_still_reads_back() {
    let router = emulator();

    // Each row differs in the members it sets, so the pass-through is not pinned on one
    // response shape.
    let cases: Vec<(&str, Value)> = vec![
        ("the defaults", json!({ "Name": "pass-through-1" })),
        (
            "an explicitly disabled accelerator",
            json!({ "Name": "pass-through-2", "Enabled": false }),
        ),
        (
            "a dual-stack accelerator carrying tags",
            json!({
                "Name": "pass-through-3",
                "IpAddressType": "DUAL_STACK",
                "Tags": [{ "Key": "owner", "Value": "team" }],
            }),
        ),
    ];

    for (case, mut body) in cases {
        body["IdempotencyToken"] = json!(format!("{case}-token"));
        let created = ok(&router, "CreateAccelerator", body).await;
        let accelerator = &created["Accelerator"];
        assert!(
            created.get(MODELLED_MESSAGE_MEMBER).is_none()
                && created.get(RENDERED_MESSAGE_MEMBER).is_none(),
            "case {case:?}: a success must carry no error text, got {created}"
        );
        let arn = accelerator["AcceleratorArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: AcceleratorArn missing from {created}"))
            .to_string();

        // The round trip: the created resource is the one a read reports.
        let read_back = ok(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_eq!(
            read_back["AcceleratorArn"], accelerator["AcceleratorArn"],
            "case {case:?}: expected the created accelerator to read back, got {read_back}"
        );
        assert_eq!(
            read_back["Name"], accelerator["Name"],
            "case {case:?}: expected the created name to read back, got {read_back}"
        );
        assert_eq!(
            read_back["Enabled"], accelerator["Enabled"],
            "case {case:?}: expected the created enablement to read back, got {read_back}"
        );
    }
}
