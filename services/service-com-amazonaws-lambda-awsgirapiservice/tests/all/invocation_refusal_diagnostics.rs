// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two sentences an INVOCATION is refused by before anything
//! runs: the one for an `InvocationType` outside its value set, and the one for a payload
//! that is not JSON.
//!
//! The rules under test:
//!
//! * a value outside a modelled enum's set is refused with the constraint sentence naming
//!   the member, the value as submitted and the whole set — and the set is listed in the
//!   order this service reports that enum in, which for `InvocationType` is `DryRun`,
//!   `RequestResponse`, `Event` and not the order the model declares its members in;
//! * a payload that is not JSON is refused with the modelled
//!   `InvalidRequestContentException` whose message carries the PARSE DIAGNOSTIC — the
//!   token or character the parse stopped on (or the input running out), what a value may
//!   begin with, and the line and column it stopped at, with the payload itself withheld —
//!   under the two fixed prefixes for the request body and the payload inside it. The same
//!   diagnostic is the one every operation taking a payload answers with, so it is asserted
//!   on `Invoke` and on `InvokeAsync`.
//!
//! Every row drives a real HTTP request through the router, so the assertions are about
//! the wire behaviour (status, error code, message), and each suite also drives the
//! ADMISSIBLE side of its rule — a modelled invocation type, a payload that is JSON — so a
//! change that refused everything would fail here.

use std::sync::Arc;

use serde_json::json;

use crate::function_lifecycle::{
    create_active_function, json_call, package_bytes, router_on, send, Answer, ROLE,
};

/// The header `Invoke$InvocationType` is bound to
/// (`smithy_query(query="operation", operation="Invoke")`: `X-Amz-Invocation-Type`).
const INVOCATION_TYPE_HEADER: &str = "x-amz-invocation-type";

/// The whole `InvocationType` value set, from
/// `smithy_query(query="shape", shape="InvocationType")`: the model declares `Event`,
/// `RequestResponse`, `DryRun`.
const INVOCATION_TYPE_VALUES: &[&str] = &["Event", "RequestResponse", "DryRun"];

/// The `InvocationType` value set as a REFUSAL lists it, which is the order this service
/// reports that enum in and NOT the model's declaration order: recorded from the AWS
/// answers for a value outside the set (corpus `simparity-wire-aws-v1`, fixtures
/// `lambda-batch-diag-submit-wrapper-function-handle-mistakes`,
/// `lambda-function-invocation-paths` and `lambda-lsrno-traj-invoke-reproduces-timeout`).
const REPORTED_VALUE_SET: &str = "DryRun, RequestResponse, Event";

/// The fixed prefixes a payload-parse refusal carries: the request body, then the payload
/// inside it.
const PARSE_PREFIXES: &str =
    "Could not parse request body into json: Could not parse payload into json: ";

/// What the location clause of a parse diagnostic names in place of the payload: the
/// payload is the caller's, so it is withheld.
const WITHHELD_SOURCE: &str =
    "REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled)";

/// The classes a JSON value may begin with, as a parse diagnostic lists them.
const VALUE_CLASSES: &str =
    "JSON String, Number, Array, Object or token 'null', 'true' or 'false'";

/// The `message` of a wire error, under either casing the JSON envelope uses.
fn message(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_else(|| String::from_utf8_lossy(&answer.body).to_string())
}

/// `Invoke` on a function, with an optional `InvocationType` header and a raw payload.
async fn invoke(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    invocation_type: Option<&str>,
    payload: &[u8],
) -> Answer {
    let headers: Vec<(&str, String)> = invocation_type
        .map(|value| vec![(INVOCATION_TYPE_HEADER, value.to_string())])
        .unwrap_or_default();
    send(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/invocations"),
        "application/octet-stream",
        payload.to_vec(),
        &headers,
    )
    .await
}

/// `InvokeAsync` on a function (`POST /2014-11-13/functions/{FunctionName}/invoke-async`),
/// whose whole body is the `InvokeArgs` payload.
async fn invoke_async(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    payload: &[u8],
) -> Answer {
    send(
        router,
        "POST",
        &format!("/2014-11-13/functions/{name}/invoke-async"),
        "application/octet-stream",
        payload.to_vec(),
        &[],
    )
    .await
}

#[tokio::test]
async fn an_invocation_type_outside_its_value_set_is_refused_with_the_whole_set() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "enum-refusal", &package_bytes(48)).await;

    // One row per form a value outside the `InvocationType` set takes: each modelled value
    // differing only in case, a word that is no invocation type at all, one that merely
    // contains a modelled value, and a value from ANOTHER of this model's enums.
    for value in [
        "requestresponse",
        "REQUESTRESPONSE",
        "event",
        "dryrun",
        "Sync",
        "RequestResponseStream",
        "Auto",
    ] {
        let refused = invoke(&router, "enum-refusal", Some(value), br#"{"k":1}"#).await;
        assert_eq!(
            refused.status,
            400,
            "case {value:?}: expected 400 from Invoke, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            message(&refused),
            format!(
                "1 validation error detected: Value '{value}' at 'invocationType' failed to \
                 satisfy constraint: Member must satisfy enum value set: [{REPORTED_VALUE_SET}]"
            ),
            "case {value:?}: expected the constraint sentence naming the value and the whole \
             set, got {:?}",
            message(&refused)
        );
    }

    // The admissible side of the same rule: every value the set DOES hold is accepted, so
    // the refusal is about the set and not about the member being read at all.
    for value in INVOCATION_TYPE_VALUES {
        let accepted = invoke(&router, "enum-refusal", Some(value), br#"{"k":1}"#).await;
        assert_ne!(
            accepted.status,
            400,
            "case {value:?}: a modelled invocation type must not be refused 400, got {} {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
    }
}

/// One payload row: what a failing assertion calls it, the payload bytes, and the
/// diagnostic the refusal must carry after the two prefixes (`None` for a payload that is
/// JSON and must not be refused at all).
struct PayloadCase {
    case: &'static str,
    payload: &'static [u8],
    diagnostic: Option<String>,
}

/// The rows span what the diagnostic generalises over: the class of fault (a bare word, a
/// character no value starts with, a word where a member NAME belongs, a container left
/// open, a payload that is only whitespace), where in the payload the fault is (its first
/// value, and a member's value), which LINE it is on, and the payloads that are JSON.
fn payload_cases() -> Vec<PayloadCase> {
    let unrecognized = |word: &str, line: usize, column: usize| {
        Some(format!(
            "Unrecognized token '{word}': was expecting ({VALUE_CLASSES})\n at [Source: \
             {WITHHELD_SOURCE}; line: {line}, column: {column}]"
        ))
    };
    vec![
        PayloadCase {
            case: "a bare word",
            payload: b"not json",
            diagnostic: unrecognized("not", 1, 1),
        },
        PayloadCase {
            case: "a bare word where a member's value belongs",
            payload: br#"{"event": created}"#,
            diagnostic: unrecognized("created", 1, 11),
        },
        PayloadCase {
            case: "a bare word on the second line",
            payload: b"{\n  \"event\": oops\n}",
            diagnostic: unrecognized("oops", 2, 12),
        },
        PayloadCase {
            case: "a character no value starts with",
            payload: b"*",
            diagnostic: Some(format!(
                "Unexpected character ('*' (code 42)): was expecting a valid value \
                 ({VALUE_CLASSES})\n at [Source: {WITHHELD_SOURCE}; line: 1, column: 1]"
            )),
        },
        PayloadCase {
            case: "an unquoted member name",
            payload: br#"{event: 1}"#,
            diagnostic: Some(format!(
                "Unexpected character ('e' (code 101)): was expecting double-quote to start \
                 field name\n at [Source: {WITHHELD_SOURCE}; line: 1, column: 2]"
            )),
        },
        PayloadCase {
            case: "an object left open",
            payload: br#"{"event": 1"#,
            diagnostic: Some(format!(
                "Unexpected end-of-input: expected close marker for Object (start marker at \
                 [Source: {WITHHELD_SOURCE}; line: 1, column: 1])\n at [Source: \
                 {WITHHELD_SOURCE}; line: 1, column: 12]"
            )),
        },
        PayloadCase {
            case: "whitespace and nothing else",
            payload: b" ",
            diagnostic: Some(format!(
                "Unexpected end-of-input: expected a value\n at [Source: {WITHHELD_SOURCE}; \
                 line: 1, column: 2]"
            )),
        },
        // The payloads that ARE JSON: an object, an array and a bare literal are each a
        // document the parse reads whole, so none of them is refused.
        PayloadCase {
            case: "an object",
            payload: br#"{"event": 1}"#,
            diagnostic: None,
        },
        PayloadCase {
            case: "an array",
            payload: b"[1, 2]",
            diagnostic: None,
        },
        PayloadCase {
            case: "a bare literal",
            payload: b"null",
            diagnostic: None,
        },
        PayloadCase {
            case: "no payload at all",
            payload: b"",
            diagnostic: None,
        },
    ]
}

#[tokio::test]
async fn a_payload_that_is_not_json_is_refused_with_the_parse_diagnostic() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "payload-refusal", &package_bytes(48)).await;

    for case in payload_cases() {
        // `Invoke` and `InvokeAsync` take the same payload and answer the same diagnostic:
        // the reading is the service's, not one operation's.
        let invoked = invoke(
            &router,
            "payload-refusal",
            Some("DryRun"),
            case.payload,
        )
        .await;
        let queued = invoke_async(&router, "payload-refusal", case.payload).await;
        for (operation, answer) in [("Invoke", &invoked), ("InvokeAsync", &queued)] {
            match &case.diagnostic {
                Some(diagnostic) => {
                    assert_eq!(
                        answer.status,
                        400,
                        "case {:?} ({operation}): expected 400, got {} {}",
                        case.case,
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert!(
                        answer.error_code().contains("InvalidRequestContentException"),
                        "case {:?} ({operation}): expected the modelled \
                         InvalidRequestContentException, got {:?}",
                        case.case,
                        answer.error_code()
                    );
                    assert_eq!(
                        message(answer),
                        format!("{PARSE_PREFIXES}{diagnostic}"),
                        "case {:?} ({operation}): expected the prefixes and the diagnostic, \
                         got {:?}",
                        case.case,
                        message(answer)
                    );
                }
                None => assert_ne!(
                    answer.status,
                    400,
                    "case {:?} ({operation}): a payload that is JSON must not be refused, got \
                     {} {}",
                    case.case,
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                ),
            }
        }
    }
}

#[tokio::test]
async fn a_refused_invocation_queues_nothing() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_active_function(&router, &engine, "refusal-queues-nothing", &package_bytes(48)).await;

    // A refusal is answered before the event is queued, so the function it named is
    // unchanged: it still reports the configuration it was created with and still accepts
    // an invocation afterwards.
    for payload in [b"not json".as_slice(), b"{".as_slice(), b"*".as_slice()] {
        let refused = invoke_async(&router, "refusal-queues-nothing", payload).await;
        assert_eq!(
            refused.status,
            400,
            "case {:?}: expected 400, got {} {}",
            String::from_utf8_lossy(payload),
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
    }

    let queued = invoke_async(&router, "refusal-queues-nothing", br#"{"event": 1}"#).await;
    assert_eq!(
        queued.status,
        202,
        "a payload that is JSON must still be queued after the refusals, got {} {}",
        queued.status,
        String::from_utf8_lossy(&queued.body)
    );

    let read = json_call(
        &router,
        "GET",
        "/2015-03-31/functions/refusal-queues-nothing/configuration",
        &json!(null),
    )
    .await;
    assert_eq!(
        (
            read.status,
            read.json()["Role"].as_str().map(str::to_string)
        ),
        (200, Some(ROLE.to_string())),
        "the function must be unchanged by the refusals, got {} {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
}
