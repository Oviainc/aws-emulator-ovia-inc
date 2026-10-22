// SPDX-License-Identifier: CC0-1.0

//! How this service states a refusal, driven over the wire.
//!
//! Two rules are under test, each over a SET of inputs rather than one request:
//!
//! 1. a request refused for violating a MODELLED input constraint is the service's own
//!    modelled client-input error — HTTP 400 `InvalidArgumentException` — carrying the
//!    constraint report built from the request (rejected value, member path, bound);
//! 2. a refusal about a named resource identifies that resource by the ARN the request
//!    carried, so two requests about two resources do not get the same sentence.
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: GlobalAccelerator_V20180706.<Operation>`, `awsJson1_1` body), so the
//! boundary validator, the routing table, the handler and the error envelope are all
//! exercised — not just a handler function.

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

fn arn_of(accelerator: &Value) -> String {
    accelerator["AcceleratorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("AcceleratorArn missing from {accelerator}"))
        .to_string()
}

/// What the boundary is expected to do with one request.
#[derive(Debug)]
enum Expect<'a> {
    /// The request violates a modelled constraint, so it is refused with HTTP 400
    /// `InvalidArgumentException` over a clause naming `path` and stating
    /// `requirement`.
    Refused {
        /// The member path the clause names (the validator's lowerCamel path).
        path: &'a str,
        /// A fragment of the requirement the clause states.
        requirement: &'a str,
    },
    /// The request satisfies every modelled constraint, so the boundary passes it to
    /// the handler: whatever the handler answers, it is not a constraint refusal.
    Accepted,
}

/// RULE: a request refused for violating a modelled input constraint is answered with
/// this service's modelled client-input error — HTTP 400 `InvalidArgumentException`,
/// which every operation taking a constrained member declares — never with a code that
/// is in no operation's error union; and the message is the constraint report built from
/// the request: the rejected value, the member's path and the bound it failed.
///
/// The constraint catalog below is the model's own, from
/// `smithy_query(query="operation", operation="CreateEndpointGroup,UpdateEndpointGroup,
/// ListAccelerators,DescribeAccelerator,CreateAccelerator,CreateListener")`:
/// `TrafficDialPercentage` @range{0,100}, `HealthCheckIntervalSeconds` @range{10,30},
/// `ThresholdCount` @range{1,10}, `HealthCheckPort` @range{1,65535},
/// `HealthCheckProtocol` enum [TCP, HTTP, HTTPS], `HealthCheckPath`
/// @pattern, `MaxResults` @range{1,100}, `GenericString` @length{0,255},
/// `PortRanges` @length{1,10}, and the members the model marks @required. Each @range is
/// exercised on both published bounds and one step past each.
#[tokio::test]
async fn a_modelled_constraint_refusal_is_the_modelled_invalid_argument_error() {
    let router = emulator();
    // A listener ARN is not resolved before the input is validated, so these rows need
    // no live resource; one is created anyway so the accepted rows exercise a real one.
    let accelerator = ok(
        &router,
        "CreateAccelerator",
        json!({ "Name": "refusals", "IdempotencyToken": "refusals-token" }),
    )
    .await["Accelerator"]
        .clone();
    let accelerator_arn = arn_of(&accelerator);
    let listener_arn = format!("{accelerator_arn}/listener/00000000");
    let endpoint_group_arn = format!("{listener_arn}/endpoint-group/00000000");
    // One past the 255-character bound `GenericString` declares.
    let overlong_arn = "a".repeat(256);

    let group = |extra: Value| -> Value {
        let mut body = json!({
            "ListenerArn": listener_arn,
            "EndpointGroupRegion": "us-west-2",
            "IdempotencyToken": "group-token",
        });
        for (key, value) in extra.as_object().expect("an object of extra members") {
            body[key] = value.clone();
        }
        body
    };
    let update_group = |extra: Value| -> Value {
        let mut body = json!({ "EndpointGroupArn": endpoint_group_arn });
        for (key, value) in extra.as_object().expect("an object of extra members") {
            body[key] = value.clone();
        }
        body
    };

    let cases: Vec<(&str, &str, Value, Expect)> = vec![
        // @range{0,100} on TrafficDialPercentage: both bounds, and one step past each.
        (
            "dial-at-the-lower-bound",
            "CreateEndpointGroup",
            group(json!({ "TrafficDialPercentage": 0.0 })),
            Expect::Accepted,
        ),
        (
            "dial-at-the-upper-bound",
            "CreateEndpointGroup",
            group(json!({ "TrafficDialPercentage": 100.0 })),
            Expect::Accepted,
        ),
        (
            "dial-below-the-lower-bound",
            "CreateEndpointGroup",
            group(json!({ "TrafficDialPercentage": -1.0 })),
            Expect::Refused {
                path: "trafficDialPercentage",
                requirement: "Member must have value greater than or equal to 0",
            },
        ),
        (
            "dial-just-past-the-upper-bound",
            "CreateEndpointGroup",
            group(json!({ "TrafficDialPercentage": 100.5 })),
            Expect::Refused {
                path: "trafficDialPercentage",
                requirement: "Member must have value less than or equal to 100",
            },
        ),
        (
            "dial-far-past-the-upper-bound",
            "CreateEndpointGroup",
            group(json!({ "TrafficDialPercentage": 150.0 })),
            Expect::Refused {
                path: "trafficDialPercentage",
                requirement: "Member must have value less than or equal to 100",
            },
        ),
        // The same member on the update leg of the same rule.
        (
            "dial-far-past-the-upper-bound-on-update",
            "UpdateEndpointGroup",
            update_group(json!({ "TrafficDialPercentage": 150.0 })),
            Expect::Refused {
                path: "trafficDialPercentage",
                requirement: "Member must have value less than or equal to 100",
            },
        ),
        (
            "dial-at-the-upper-bound-on-update",
            "UpdateEndpointGroup",
            update_group(json!({ "TrafficDialPercentage": 100.0 })),
            Expect::Accepted,
        ),
        // @range{10,30} on HealthCheckIntervalSeconds.
        (
            "interval-at-the-bounds",
            "CreateEndpointGroup",
            group(json!({ "HealthCheckIntervalSeconds": 10 })),
            Expect::Accepted,
        ),
        (
            "interval-below-the-lower-bound",
            "CreateEndpointGroup",
            group(json!({ "HealthCheckIntervalSeconds": 9 })),
            Expect::Refused {
                path: "healthCheckIntervalSeconds",
                requirement: "Member must have value greater than or equal to 10",
            },
        ),
        (
            "interval-past-the-upper-bound",
            "CreateEndpointGroup",
            group(json!({ "HealthCheckIntervalSeconds": 31 })),
            Expect::Refused {
                path: "healthCheckIntervalSeconds",
                requirement: "Member must have value less than or equal to 30",
            },
        ),
        // @range{1,10} on ThresholdCount.
        (
            "threshold-below-the-lower-bound",
            "CreateEndpointGroup",
            group(json!({ "ThresholdCount": 0 })),
            Expect::Refused {
                path: "thresholdCount",
                requirement: "Member must have value greater than or equal to 1",
            },
        ),
        (
            "threshold-past-the-upper-bound",
            "CreateEndpointGroup",
            group(json!({ "ThresholdCount": 11 })),
            Expect::Refused {
                path: "thresholdCount",
                requirement: "Member must have value less than or equal to 10",
            },
        ),
        // @range{1,65535} on HealthCheckPort.
        (
            "health-check-port-past-the-upper-bound",
            "CreateEndpointGroup",
            group(json!({ "HealthCheckPort": 65536 })),
            Expect::Refused {
                path: "healthCheckPort",
                requirement: "Member must have value less than or equal to 65535",
            },
        ),
        // An enumerated member: a value outside the model's value set.
        (
            "health-check-protocol-outside-the-value-set",
            "CreateEndpointGroup",
            group(json!({ "HealthCheckProtocol": "SMTP" })),
            Expect::Refused {
                path: "healthCheckProtocol",
                requirement: "enum value set",
            },
        ),
        (
            "health-check-protocol-in-the-value-set",
            "CreateEndpointGroup",
            group(json!({ "HealthCheckProtocol": "HTTPS" })),
            Expect::Accepted,
        ),
        // A @pattern member: a path that does not start with a slash.
        (
            "health-check-path-outside-the-pattern",
            "CreateEndpointGroup",
            group(json!({ "HealthCheckPath": "health" })),
            Expect::Refused {
                path: "healthCheckPath",
                requirement: "regular expression pattern",
            },
        ),
        // @range{1,100} on MaxResults, on an implemented operation: both bounds and one
        // step past each.
        (
            "max-results-at-the-lower-bound",
            "ListAccelerators",
            json!({ "MaxResults": 1 }),
            Expect::Accepted,
        ),
        (
            "max-results-at-the-upper-bound",
            "ListAccelerators",
            json!({ "MaxResults": 100 }),
            Expect::Accepted,
        ),
        (
            "max-results-below-the-lower-bound",
            "ListAccelerators",
            json!({ "MaxResults": 0 }),
            Expect::Refused {
                path: "maxResults",
                requirement: "Member must have value greater than or equal to 1",
            },
        ),
        (
            "max-results-past-the-upper-bound",
            "ListAccelerators",
            json!({ "MaxResults": 101 }),
            Expect::Refused {
                path: "maxResults",
                requirement: "Member must have value less than or equal to 100",
            },
        ),
        // @length{0,255} on a string member.
        (
            "accelerator-arn-past-the-length-bound",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": overlong_arn }),
            Expect::Refused {
                path: "acceleratorArn",
                requirement: "Member must have length less than or equal to 255",
            },
        ),
        // @length{1,10} on a list member: an empty list names no port range at all.
        (
            "port-ranges-below-the-length-bound",
            "CreateListener",
            json!({
                "AcceleratorArn": accelerator_arn,
                "PortRanges": [],
                "Protocol": "TCP",
                "IdempotencyToken": "empty-ranges",
            }),
            Expect::Refused {
                path: "portRanges",
                requirement: "Member must have length greater than or equal to 1",
            },
        ),
        // @required members, on an implemented and on a scaffolded operation.
        (
            "accelerator-name-absent",
            "CreateAccelerator",
            json!({ "IdempotencyToken": "no-name" }),
            Expect::Refused {
                path: "name",
                requirement: "Member must not be null",
            },
        ),
        (
            "endpoint-group-region-absent",
            "CreateEndpointGroup",
            json!({ "ListenerArn": listener_arn, "IdempotencyToken": "no-region" }),
            Expect::Refused {
                path: "endpointGroupRegion",
                requirement: "Member must not be null",
            },
        ),
    ];

    for (case, operation, body, expectation) in cases {
        let (status, parsed) = call(&router, operation, body).await;
        let code = error_code(&parsed);
        let message = error_message(&parsed);
        match expectation {
            Expect::Refused { path, requirement } => {
                assert_eq!(
                    (status, code.as_str()),
                    (400, "InvalidArgumentException"),
                    "case {case}: expected the modelled 400 InvalidArgumentException, \
                     got {status} {parsed}"
                );
                assert!(
                    message.contains(&format!("at '{path}'")),
                    "case {case}: expected the clause to name the member {path:?}, \
                     got {message:?}"
                );
                assert!(
                    message.contains(requirement),
                    "case {case}: expected the clause to state {requirement:?}, \
                     got {message:?}"
                );
                assert!(
                    message.starts_with("1 validation error detected: "),
                    "case {case}: expected the constraint-report envelope, got {message:?}"
                );
            }
            Expect::Accepted => {
                assert!(
                    code != "InvalidArgumentException" && code != "ValidationException",
                    "case {case}: a value inside every modelled bound must reach the \
                     handler, got {status} {parsed}"
                );
            }
        }
    }
}

/// RULE: a refusal about a resource the request named identifies that resource by the
/// ARN the request carried, so the diagnostic can be attributed to one resource out of
/// many; and a refused delete changes nothing — the accelerator is still there, still
/// enabled.
#[tokio::test]
async fn a_refusal_names_the_resource_the_request_identified() {
    let router = emulator();

    // Several accelerators, so the message is tested as a function of the request rather
    // than on one value: each row differs in name, in whether it carries a listener, and
    // therefore in which refusal its delete meets.
    let cases: Vec<(&str, &str, bool)> = vec![
        ("first", "alpha", false),
        ("second", "beta", true),
        ("third", "gamma.01", false),
    ];

    let mut seen_messages: Vec<String> = Vec::new();
    for (case, name, with_listener) in cases {
        let arn = arn_of(
            &ok(
                &router,
                "CreateAccelerator",
                json!({ "Name": name, "IdempotencyToken": format!("{case}-token"), "Enabled": true }),
            )
            .await["Accelerator"],
        );
        if with_listener {
            ok(
                &router,
                "CreateListener",
                json!({
                    "AcceleratorArn": arn,
                    "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
                    "Protocol": "TCP",
                    "IdempotencyToken": format!("{case}-listener"),
                }),
            )
            .await;
        }

        // Enabled: the delete is refused, and the refusal names this accelerator.
        let (status, parsed) = call(&router, "DeleteAccelerator", json!({ "AcceleratorArn": arn })).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "AcceleratorNotDisabledException"),
            "case {case}: expected the modelled refusal for an enabled accelerator, \
             got {status} {parsed}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message,
            format!("Please disable accelerator {arn} before deleting it."),
            "case {case}: expected the refusal to name the accelerator, got {message:?}"
        );
        assert!(
            !seen_messages.contains(&message),
            "case {case}: two accelerators must not get the same refusal, got {message:?}"
        );
        seen_messages.push(message);

        // The refused delete is a no-op.
        let reread = ok(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await["Accelerator"]
            .clone();
        assert_eq!(
            reread["Enabled"],
            json!(true),
            "case {case}: a refused delete must change nothing, got {reread}"
        );

        ok(
            &router,
            "UpdateAccelerator",
            json!({ "AcceleratorArn": arn, "Enabled": false }),
        )
        .await;

        if with_listener {
            // The next precondition, stated over the same ARN.
            let (status, parsed) =
                call(&router, "DeleteAccelerator", json!({ "AcceleratorArn": arn })).await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "AssociatedListenerFoundException"),
                "case {case}: expected the modelled refusal for a listener still \
                 attached, got {status} {parsed}"
            );
            let message = error_message(&parsed);
            assert!(
                message.contains(&arn),
                "case {case}: expected the refusal to name the accelerator, got {message:?}"
            );

            let listeners = ok(&router, "ListListeners", json!({ "AcceleratorArn": arn })).await
                ["Listeners"]
                .clone();
            for listener in listeners.as_array().cloned().unwrap_or_default() {
                ok(
                    &router,
                    "DeleteListener",
                    json!({ "ListenerArn": listener["ListenerArn"] }),
                )
                .await;
            }
        }

        // Disabled and empty: the delete goes through, and the accelerator is gone.
        ok(&router, "DeleteAccelerator", json!({ "AcceleratorArn": arn })).await;
        let (status, parsed) = call(
            &router,
            "DescribeAccelerator",
            json!({ "AcceleratorArn": arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (404, "AcceleratorNotFoundException"),
            "case {case}: expected the modelled not-found after the delete, \
             got {status} {parsed}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message,
            format!("Accelerator {arn} not found."),
            "case {case}: expected the not-found to name the accelerator, got {message:?}"
        );
    }
}

/// RULE: a not-found refusal names the resource the request identified, at every level
/// of the hierarchy — an accelerator ARN on the accelerator operations, a listener ARN
/// on the listener operations.
#[tokio::test]
async fn a_not_found_refusal_names_the_arn_the_request_carried() {
    let router = emulator();
    let missing_accelerator =
        "arn:aws:globalaccelerator::000000000000:accelerator/00000000-0000-4000-8000-000000000000";
    let other_accelerator =
        "arn:aws:globalaccelerator::000000000000:accelerator/11111111-1111-4111-8111-111111111111";
    let missing_listener = format!("{missing_accelerator}/listener/deadbeef");

    // (case, operation, body, expected message)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "describe-accelerator",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": missing_accelerator }),
            format!("Accelerator {missing_accelerator} not found."),
        ),
        (
            "describe-another-accelerator",
            "DescribeAccelerator",
            json!({ "AcceleratorArn": other_accelerator }),
            format!("Accelerator {other_accelerator} not found."),
        ),
        (
            "delete-accelerator",
            "DeleteAccelerator",
            json!({ "AcceleratorArn": missing_accelerator }),
            format!("Accelerator {missing_accelerator} not found."),
        ),
        (
            "update-accelerator",
            "UpdateAccelerator",
            json!({ "AcceleratorArn": missing_accelerator, "Enabled": false }),
            format!("Accelerator {missing_accelerator} not found."),
        ),
        (
            "list-listeners",
            "ListListeners",
            json!({ "AcceleratorArn": missing_accelerator }),
            format!("Accelerator {missing_accelerator} not found."),
        ),
        (
            "describe-listener",
            "DescribeListener",
            json!({ "ListenerArn": missing_listener }),
            format!("Listener {missing_listener} not found."),
        ),
        (
            "delete-listener",
            "DeleteListener",
            json!({ "ListenerArn": missing_listener }),
            format!("Listener {missing_listener} not found."),
        ),
    ];

    for (case, operation, body, expected) in cases {
        let (status, parsed) = call(&router, operation, body).await;
        assert_eq!(
            status, 404,
            "case {case}: expected the modelled 404, got {status} {parsed}"
        );
        let message = error_message(&parsed);
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}
