// SPDX-License-Identifier: CC0-1.0

//! Which identifier an ARN-addressed `ResourceNotFoundException` names.
//!
//! The rule, asserted over a class of caller-supplied strings rather than over
//! one request: an identifier is lifted out of a caller string only when that
//! whole string addresses a tunnel — a well-formed resource ARN (read through
//! `service_common::arn`, the workspace's one caller-supplied-ARN parse) whose
//! namespace is the `iot` one a tunnel is addressed in, whose Region label and
//! account are the ones that own the regional resource, and whose resource is a
//! `tunnel` carrying an id `TunnelId` admits. A string failing any of those
//! names no tunnel resource, so the only identifier the request offered is the
//! string itself and the message names that.
//!
//! Driven over the wire through the `MultiServiceRouter`, for each of the three
//! ARN-addressed operations (`ListTagsForResource`, `TagResource`,
//! `UntagResource`), exactly as a client reaches the service.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test signs for, so a string's Region segment is observable.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// A router carrying only this service.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    call_signed_for(router, REGION, operation, input).await
}

/// [`call`], with the Region the request's credential scope names spelled out —
/// the one the service files and addresses its records by.
async fn call_signed_for(
    router: &MultiServiceRouter,
    scope_region: &str,
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
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{scope_region}/\
                 IoTSecuredTunneling/aws4_request, SignedHeaders=host, Signature=abc"
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

/// The `message` member of a refusal body.
fn message(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// Open one tunnel in `REGION` and return `(tunnelId, tunnelArn)`.
async fn open_tunnel(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let input = json!({
        "description": case,
        "destinationConfig": {"thingName": "thing-lifting", "services": ["SSH"]},
    });
    let (status, opened) = call(router, "OpenTunnel", &input).await;
    assert_eq!(status, 200, "case {case:?}: OpenTunnel answered {opened:?}");
    (
        opened["tunnelId"].as_str().unwrap_or_default().to_string(),
        opened["tunnelArn"].as_str().unwrap_or_default().to_string(),
    )
}

/// What the refusal's message must name for one caller-supplied string.
#[derive(Debug)]
enum Names {
    /// The string addresses a tunnel, so the id lifted out of its resource
    /// segment.
    LiftedId(String),
    /// The string addresses no tunnel resource, so the string itself.
    SuppliedString,
}

/// Every ARN-addressed request form, over one caller-supplied string.
fn arn_addressed_calls(arn: &str) -> Vec<(&'static str, Value)> {
    vec![
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

#[tokio::test]
async fn an_id_is_lifted_only_out_of_a_string_that_addresses_a_tunnel() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    // A real tunnel exists throughout, so each refusal below is about the string
    // supplied and not about an empty store.
    let (live_id, live_arn) = open_tunnel(&router, "the live tunnel").await;

    // `TunnelId`'s modelled pattern, from
    // `smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#TunnelId")`:
    // `^[a-zA-Z0-9_\-+=:]{1,128}$` — so the rows below walk its punctuation,
    // each published length bound and one past the ceiling.
    let longest_id = "z".repeat(128);
    let too_long_id = "z".repeat(129);

    let rows: Vec<(&str, String, Names)> = vec![
        // ---- strings that DO address a tunnel: the id is lifted ----
        (
            "a well-formed tunnel ARN of this Region naming no tunnel",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/0f1e2d3c-4b5a-4987-8765-432112345678"),
            Names::LiftedId("0f1e2d3c-4b5a-4987-8765-432112345678".to_string()),
        ),
        (
            "another Region's tunnel ARN",
            format!("arn:aws:iot:us-west-2:{ACCOUNT}:tunnel/abc-123"),
            Names::LiftedId("abc-123".to_string()),
        ),
        (
            "another account's tunnel ARN",
            format!("arn:aws:iot:{REGION}:111111111111:tunnel/abc-123"),
            Names::LiftedId("abc-123".to_string()),
        ),
        (
            "another partition's tunnel ARN",
            "arn:aws-cn:iot:cn-north-1:000000000000:tunnel/abc-123".to_string(),
            Names::LiftedId("abc-123".to_string()),
        ),
        (
            "an id using every character class the pattern admits, ':' included",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/weird_id+with=every:char-class"),
            Names::LiftedId("weird_id+with=every:char-class".to_string()),
        ),
        (
            "the shortest id the pattern admits",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/a"),
            Names::LiftedId("a".to_string()),
        ),
        (
            // `service_common::arn` reads a resource in both of the forms AWS
            // writes one in, `<type>/<id>` and `<type>:<id>`.
            "the resource written in the colon form",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel:abc-123"),
            Names::LiftedId("abc-123".to_string()),
        ),
        (
            "the longest id the pattern admits",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/{longest_id}"),
            Names::LiftedId(longest_id.clone()),
        ),
        // ---- strings that address NO tunnel: the supplied string is named ----
        (
            "another service's namespace",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:tunnel/abc"),
            Names::SuppliedString,
        ),
        (
            // The model's `aws.api#service$arnNamespace` metadata, which is the
            // internal service name and not the namespace a tunnel ARN carries.
            "the internal service name as the namespace",
            format!("arn:aws:iotsecuredtunneling:{REGION}:{ACCOUNT}:tunnel/abc"),
            Names::SuppliedString,
        ),
        (
            "six fields with no 'arn' marker",
            "a:b:c:d:e:tunnel/abc".to_string(),
            Names::SuppliedString,
        ),
        (
            "an empty partition",
            format!("arn::iot:{REGION}:{ACCOUNT}:tunnel/abc"),
            Names::SuppliedString,
        ),
        (
            "an empty Region",
            format!("arn:aws:iot::{ACCOUNT}:tunnel/abc"),
            Names::SuppliedString,
        ),
        (
            // A Region field is a label of `[a-z0-9-]`, bounded in length — the
            // grammar every model's ARN pattern states for the field.
            "a Region field that is not a Region label",
            format!("arn:aws:iot:not a region:{ACCOUNT}:tunnel/abc"),
            Names::SuppliedString,
        ),
        (
            "a Region field past the longest label an endpoint has",
            format!("arn:aws:iot:eu-west-1-eu-west-1-eu:{ACCOUNT}:tunnel/abc"),
            Names::SuppliedString,
        ),
        (
            "an empty account",
            format!("arn:aws:iot:{REGION}::tunnel/abc"),
            Names::SuppliedString,
        ),
        (
            "another resource type in the tunnel's namespace",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:thing/thing-1"),
            Names::SuppliedString,
        ),
        (
            "the resource type with no id after it",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/"),
            Names::SuppliedString,
        ),
        (
            "the resource type with no separator",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel"),
            Names::SuppliedString,
        ),
        (
            "an id carrying a character the pattern excludes",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/abc def"),
            Names::SuppliedString,
        ),
        (
            "an id one character past the pattern's ceiling",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}:tunnel/{too_long_id}"),
            Names::SuppliedString,
        ),
        (
            "fewer than six colon-separated fields",
            format!("arn:aws:iot:{REGION}:{ACCOUNT}"),
            Names::SuppliedString,
        ),
        (
            "no colon at all",
            "just-a-string".to_string(),
            Names::SuppliedString,
        ),
    ];

    for (label, arn, names) in &rows {
        let identifier = match names {
            Names::LiftedId(id) => id.as_str(),
            Names::SuppliedString => arn.as_str(),
        };
        let expected = format!("Tunnel {identifier} does not exist.");
        for (operation, input) in arn_addressed_calls(arn) {
            let case = format!("{label} ({arn:?}) through {operation}");
            let (status, body) = call(&router, operation, &input).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, answered {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: answered {body:?}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                message(&body)
            );
        }
    }

    // None of those refusals resolved anything into existence or removed the one
    // real tunnel: its own ARN still addresses it.
    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": live_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {live_arn:?}: the live tunnel's ARN must still resolve, answered {status} {listed:?}"
    );
    let (status, described) = call(&router, "DescribeTunnel", &json!({"tunnelId": live_id})).await;
    assert_eq!(
        status, 200,
        "case {live_id:?}: the live tunnel must still be readable, answered {status} {described:?}"
    );
}

#[tokio::test]
async fn a_string_that_addresses_the_live_tunnel_tags_it_round_trip() {
    // The companion of the rule: tightening WHICH strings carry a liftable id
    // leaves the strings that really address a tunnel resolving as before, so a
    // tag written through the ARN reads back through the ARN.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);
    let (_, live_arn) = open_tunnel(&router, "the tagged tunnel").await;

    let (status, tagged) = call(
        &router,
        "TagResource",
        &json!({"resourceArn": live_arn, "tags": [{"key": "env", "value": "test"}]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {live_arn:?}: TagResource answered {status} {tagged:?}"
    );

    let (status, listed) = call(
        &router,
        "ListTagsForResource",
        &json!({"resourceArn": live_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {live_arn:?}: ListTagsForResource answered {status} {listed:?}"
    );
    assert_eq!(
        listed["tags"],
        json!([{"key": "env", "value": "test"}]),
        "case {live_arn:?}: expected the stored tag, got {:?}",
        listed["tags"]
    );
}

#[tokio::test]
async fn every_minted_arn_is_one_the_arn_addressed_operations_can_read_back() {
    // The other half of the same rule: a tunnel is filed and its ARN minted
    // under a Region LABEL, whatever the credential scope spells, so the ARN a
    // create hands back always resolves through the ARN-addressed operations.
    // The rows are the scope Regions a request can carry — labels of several
    // forms, and values that are no Region label at all.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let scopes = [
        ("a Region label", REGION),
        ("another Region label", "ap-southeast-2"),
        ("a Region label carrying digits", "us-gov-west-1"),
        ("an upper-case value, which is no Region label", "EU-WEST-1"),
        ("a value carrying a separator", "eu-west-1/x"),
        ("a value past the longest label an endpoint has", "eu-west-1-eu-west-1-eu"),
    ];

    for (label, scope_region) in scopes {
        let case = format!("{label} ({scope_region:?})");
        let (status, opened) = call_signed_for(
            &router,
            scope_region,
            "OpenTunnel",
            &json!({
                "description": label,
                "destinationConfig": {"thingName": "thing-scope", "services": ["SSH"]},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: OpenTunnel answered {status} {opened:?}"
        );
        let arn = opened["tunnelArn"].as_str().unwrap_or_default().to_string();

        let (status, listed) = call_signed_for(
            &router,
            scope_region,
            "ListTagsForResource",
            &json!({"resourceArn": arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the ARN {arn:?} the create minted must resolve, answered {status} \
             {listed:?}"
        );
    }
}
