// SPDX-License-Identifier: CC0-1.0

//! The envelope every refused invocation of this data plane is stated in.
//!
//! Three rules, each asserted over a TABLE rather than over one recorded call:
//!
//! * a refusal names the error SHAPE the model declares, followed by the service stack's
//!   own locator, in the `x-amzn-errortype` header a restJson1 client resolves the shape
//!   from — on every route and for every refusal class;
//! * the refusal of an endpoint the account does not host carries the endpoint-lookup
//!   discriminator `ErrorCode` beside the modelled `Message`, on all three routes;
//! * a request outside a MODELLED constraint is refused through the modelled
//!   `ValidationError`, whose `Message` carries the AWS validation envelope naming the
//!   member and quoting the constraint as the model declares it.
//!
//! The body-size rows pin the ordering rule the routes share: the endpoint a request is
//! addressed to is resolved before the payload the service streams to the model container
//! is examined, while the payload that is one of the asynchronous route's two input
//! sources is examined with the choice between them, before the endpoint resolves.

use serde_json::Value;

use crate::harness::{
    async_invocations, headers_for, invocations, post, response_stream, routes, Emulator, Headers,
    ACCOUNT,
};

/// One refusal row: the case, the path it calls, the headers it sends, and the clause the
/// refusal must state.
type ClauseRow<'r> = (&'r str, String, Headers<'r>, String);

/// One failure row: the case, the path, the headers, the status and the error shape the
/// answer must name.
type ShapeRow<'r> = (&'r str, String, Headers<'r>, u16, &'r str);

/// The shape every client error of this service resolves to: `ValidationError` is the one
/// client-input error the model declares (`smithy_query(query="shape",
/// shape="ValidationError")`).
const VALIDATION_ERROR: &str = "ValidationError";

/// The whole `x-amzn-errortype` value this service answers that shape with: the shape, the
/// separator, and the service stack's own locator for it.
const QUALIFIED_VALIDATION_ERROR: &str =
    "ValidationError:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/";

/// The discriminator the endpoint-lookup refusal carries.
const NO_SUCH_ENDPOINT: &str = "NO_SUCH_ENDPOINT";

/// `BodyBlob`'s modelled `@length` maximum, which `InvokeEndpoint$Body` and
/// `InvokeEndpointWithResponseStream$Body` both target (`smithy_query(query="shape",
/// shape="BodyBlob")`), and `AsyncBodyBlob`'s, which `InvokeEndpointAsync$Body` targets.
const BODY_MAX: usize = 6_291_456;
const ASYNC_BODY_MAX: usize = 128_000;

/// The maximum the model declares for the payload of `operation`.
fn payload_maximum(operation: &str) -> usize {
    match operation {
        "InvokeEndpointAsync" => ASYNC_BODY_MAX,
        _ => BODY_MAX,
    }
}

/// One member of the response body, or `None` when the body carries none.
fn member(call: &crate::harness::Call, name: &str) -> Option<String> {
    call.json()
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// RULE: an endpoint the account does not host is refused, on every route, with the
/// modelled `ValidationError` carrying BOTH the sentence that names the endpoint and the
/// account and the fixed `ErrorCode` discriminator a client branches on.
///
/// The rows differ along the route and along the shape of the name the request addressed —
/// one character, hyphenated, all digits, and the 63-character modelled maximum — because
/// the rule is about an endpoint that does not exist, not about one recorded name.
#[tokio::test]
async fn an_endpoint_the_account_does_not_host_is_refused_with_the_lookup_discriminator() {
    let emulator = Emulator::new();
    // `EndpointName` is `len 0..63`, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`
    // (smithy_query(query="shape", shape="EndpointName")).
    let names = [
        "a",
        "9",
        "absent-endpoint-1",
        "AbsentEndpoint",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let routes = routes();

    for (operation, path, headers) in routes {
        for name in names {
            let case = format!("{operation} / {name}");
            let call = post(
                &emulator,
                &path(name),
                &headers_for(&headers, b"payload"),
                b"payload",
            )
            .await;
            assert_eq!(
                call.status, 400,
                "case {case}: expected HTTP 400, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_type().as_deref(),
                Some(QUALIFIED_VALIDATION_ERROR),
                "case {case}: expected the qualified modelled shape in x-amzn-errortype, got {:?}",
                call.error_type()
            );
            assert_eq!(
                member(&call, "Message"),
                Some(format!("Endpoint {name} of account {ACCOUNT} not found.")),
                "case {case}: expected the modelled Message member, got {:?}",
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                member(&call, "ErrorCode").as_deref(),
                Some(NO_SUCH_ENDPOINT),
                "case {case}: expected the endpoint-lookup discriminator, got {:?}",
                String::from_utf8_lossy(&call.body)
            );
        }
    }
}

/// RULE: a request carrying a value outside a MODELLED constraint is refused through the
/// modelled `ValidationError` — the AWS validation envelope in that shape's own `Message`
/// member, naming the member in its lowerCamel wire name and quoting the constraint as the
/// model declares it — and carries no `ErrorCode`, which belongs to the endpoint lookup
/// alone.
///
/// The rows span the constraint classes the model declares on these inputs (`@pattern`,
/// `@length` and `@range`), the members they sit on (a `@httpLabel` and four
/// `@httpHeader`s) and two routes, so the rule is not keyed to one member or one route.
#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_refused_in_the_modelled_envelope() {
    let emulator = Emulator::new();
    // Each row: the case, the path, the headers, and the clause the refusal must state.
    // Every expression is the model's (smithy_query(query="shape",
    // shape="InvokeEndpointInput") / "InvokeEndpointAsyncInput"), quoted the way this
    // service quotes it: the family's resource-name expression is anchored at the start
    // only, every other expression as the model writes it.
    let long_name = "a".repeat(64);
    // One character past `CustomAttributesHeader`'s modelled `@length` maximum (0..1024).
    let sensitive_over_length = "s".repeat(1025);
    let rows: Vec<ClauseRow<'_>> = vec![
        (
            "endpointName pattern",
            invocations("sim2real_probe_bad_name"),
            vec![],
            "Value 'sim2real_probe_bad_name' at 'endpointName' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*"
                .to_string(),
        ),
        (
            "endpointName length",
            invocations(&long_name),
            vec![],
            format!(
                "Value '{long_name}' at 'endpointName' failed to satisfy constraint: Member \
                 must have length less than or equal to 63"
            ),
        ),
        (
            "targetVariant pattern",
            invocations("absent-endpoint"),
            vec![("x-amzn-sagemaker-target-variant", "not_a_variant")],
            "Value 'not_a_variant' at 'targetVariant' failed to satisfy constraint: Member \
             must satisfy regular expression pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9])*"
                .to_string(),
        ),
        (
            "sessionId pattern",
            invocations("absent-endpoint"),
            vec![("x-amzn-sagemaker-session-id", "not_a_session_id")],
            "Value 'not_a_session_id' at 'sessionId' failed to satisfy constraint: Member \
             must satisfy regular expression pattern: \
             ^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$"
                .to_string(),
        ),
        (
            "inputLocation pattern",
            async_invocations("absent-endpoint"),
            vec![(
                "x-amzn-sagemaker-inputlocation",
                "ftp://bucket/input/payload.json",
            )],
            "Value 'ftp://bucket/input/payload.json' at 'inputLocation' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: \
             ^(https|s3)://([^/]+)/?(.*)$"
                .to_string(),
        ),
        (
            "requestTTLSeconds range",
            async_invocations("absent-endpoint"),
            vec![
                crate::harness::INPUT_LOCATION,
                ("x-amzn-sagemaker-requestttlseconds", "59"),
            ],
            "Value '59' at 'requestTTLSeconds' failed to satisfy constraint: Member must \
             have value greater than or equal to 60"
                .to_string(),
        ),
        // `CustomAttributes` is `@sensitive` (smithy_query(query="shape",
        // shape="InvokeEndpointInput")), so its clause names the member and the
        // requirement and states NO value: a refusal that echoed it would disclose what
        // the caller forwards to the model container into every log the response reaches.
        (
            "customAttributes length, withheld",
            invocations("absent-endpoint"),
            vec![(
                "x-amzn-sagemaker-custom-attributes",
                sensitive_over_length.as_str(),
            )],
            "Value at 'customAttributes' failed to satisfy constraint: Member must have \
             length less than or equal to 1024"
                .to_string(),
        ),
    ];

    for (case, path, headers, clause) in rows {
        let call = post(&emulator, &path, &headers, b"payload").await;
        assert_eq!(
            call.status, 400,
            "case {case}: expected HTTP 400, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_type().as_deref(),
            Some(QUALIFIED_VALIDATION_ERROR),
            "case {case}: expected the qualified modelled shape in x-amzn-errortype, got {:?}",
            call.error_type()
        );
        assert_eq!(
            member(&call, "Message"),
            Some(format!("1 validation error detected: {clause}")),
            "case {case}: expected the AWS envelope in the modelled Message member, got {:?}",
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            member(&call, "ErrorCode"),
            None,
            "case {case}: a constraint refusal carries no endpoint-lookup discriminator, got {:?}",
            String::from_utf8_lossy(&call.body)
        );
    }
}

/// RULE: the endpoint the request is addressed to is resolved BEFORE the payload the
/// service streams to the model container is examined, so a request naming an endpoint the
/// account does not host is refused for that whatever its payload carries — while the
/// asynchronous route's payload is one of two mutually exclusive INPUT SOURCES, so its
/// modelled ceiling belongs to choosing the source and is examined before the endpoint
/// resolves.
///
/// The rows walk each route's own modelled payload range — the bounds the model publishes,
/// a value inside it and one past the maximum — on all three routes.
#[tokio::test]
async fn the_endpoint_is_resolved_before_the_payload_is_examined() {
    let emulator = Emulator::new();
    for (operation, path, headers) in routes() {
        let maximum = payload_maximum(operation);
        // `InvokeEndpointAsyncInput` declares `Body` and `InputLocation` both optional and
        // mutually exclusive, so the inline payload is a source the request layer chooses
        // between; the other two routes declare `Body` `@required` (`@clientOptional`) and
        // pass it to the model container, so it is examined once the addressing resolved.
        let inline_source = operation == "InvokeEndpointAsync";
        for size in [0usize, 1, maximum - 1, maximum, maximum + 1] {
            let case = format!("{operation} / {size} bytes");
            let body = vec![b'x'; size];
            let call = post(
                &emulator,
                &path("absent-endpoint"),
                &headers_for(&headers, &body),
                &body,
            )
            .await;
            assert_eq!(
                call.status, 400,
                "case {case}: expected HTTP 400, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            if inline_source && size > maximum {
                assert_eq!(
                    member(&call, "Message"),
                    Some(format!(
                        "1 validation error detected: Value at 'body' failed to satisfy \
                         constraint: Member must have length less than or equal to {maximum}"
                    )),
                    "case {case}: an inline input source past its modelled ceiling is refused \
                     before the endpoint resolves, got {:?}",
                    String::from_utf8_lossy(&call.body)
                );
                assert_eq!(
                    member(&call, "ErrorCode"),
                    None,
                    "case {case}: a constraint refusal carries no endpoint-lookup \
                     discriminator, got {:?}",
                    String::from_utf8_lossy(&call.body)
                );
                continue;
            }
            assert_eq!(
                member(&call, "Message"),
                Some(format!(
                    "Endpoint absent-endpoint of account {ACCOUNT} not found."
                )),
                "case {case}: the endpoint lookup must answer first, got {:?}",
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                member(&call, "ErrorCode").as_deref(),
                Some(NO_SUCH_ENDPOINT),
                "case {case}: expected the endpoint-lookup discriminator, got {:?}",
                String::from_utf8_lossy(&call.body)
            );
        }
    }
}

/// RULE: a payload past the modelled maximum is refused once the endpoint it addressed
/// resolves — in the modelled envelope, naming the payload member and the bound it broke,
/// and stating no value, because the payload is `@sensitive` — and a payload inside the
/// maximum gets past validation altogether.
#[tokio::test]
async fn a_payload_past_the_modelled_maximum_is_refused_once_the_endpoint_resolves() {
    let emulator = Emulator::new();
    emulator
        .deploy("sized", &["AllTraffic"], serde_json::json!({}))
        .await;

    for size in [0usize, 1, BODY_MAX] {
        let call = post(&emulator, &invocations("sized"), &[], &vec![b'x'; size]).await;
        assert_eq!(
            call.status, 424,
            "case {size} bytes: a payload inside the modelled maximum must reach the \
             inference step, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
    }

    let over = post(
        &emulator,
        &invocations("sized"),
        &[],
        &vec![b'x'; BODY_MAX + 1],
    )
    .await;
    assert_eq!(
        over.status, 400,
        "case {} bytes: a payload past the modelled maximum is refused, got {} / {}",
        BODY_MAX + 1,
        over.status,
        String::from_utf8_lossy(&over.body)
    );
    assert_eq!(
        over.error_type().as_deref(),
        Some(QUALIFIED_VALIDATION_ERROR),
        "case over the maximum: expected the qualified modelled shape, got {:?}",
        over.error_type()
    );
    assert_eq!(
        member(&over, "Message"),
        Some(format!(
            "1 validation error detected: Value at 'body' failed to satisfy constraint: \
             Member must have length less than or equal to {BODY_MAX}"
        )),
        "case over the maximum: expected the AWS envelope naming the payload member, got {:?}",
        String::from_utf8_lossy(&over.body)
    );
    assert!(
        !String::from_utf8_lossy(&over.body).contains("xxxx"),
        "case over the maximum: the payload is @sensitive, so no part of it is echoed, got {:?}",
        String::from_utf8_lossy(&over.body)
    );
}

/// RULE: every failure this service answers names its error shape with the locator — not
/// only the client errors — so one client-side rule reads the header for all of them.
#[tokio::test]
async fn every_failure_names_its_shape_with_the_locator() {
    let emulator = Emulator::new();
    emulator
        .deploy("named", &["AllTraffic"], serde_json::json!({}))
        .await;

    // Each row: the case, the route, the headers, the status and the shape it must name.
    let rows: Vec<ShapeRow<'_>> = vec![
        (
            "no such endpoint",
            invocations("absent-endpoint"),
            vec![],
            400,
            VALIDATION_ERROR,
        ),
        (
            "no container served the inference",
            invocations("named"),
            vec![],
            424,
            "ModelError",
        ),
        (
            "no container served the streamed inference",
            response_stream("named"),
            vec![],
            424,
            "ModelError",
        ),
        (
            "a variant the endpoint does not host",
            invocations("named"),
            vec![("x-amzn-sagemaker-target-variant", "Elsewhere")],
            400,
            VALIDATION_ERROR,
        ),
    ];

    for (case, path, headers, status, shape) in rows {
        let call = post(&emulator, &path, &headers, b"payload").await;
        assert_eq!(
            call.status, status,
            "case {case}: expected HTTP {status}, got {} / {}",
            call.status,
            String::from_utf8_lossy(&call.body)
        );
        assert_eq!(
            call.error_type(),
            Some(format!(
                "{shape}:http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/"
            )),
            "case {case}: expected the qualified shape in x-amzn-errortype, got {:?}",
            call.error_type()
        );
    }
}
