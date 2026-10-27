// SPDX-License-Identifier: CC0-1.0

//! How the ARN-rejection sentence QUOTES BACK a value longer than the echo ceiling,
//! whichever layer of this service words it.
//!
//! The rule pinned here is one sentence for the whole class:
//!
//! > `Invalid ARN for <resource type>: <value>` renders the submitted value through ONE
//! > echo ceiling — the value itself at or under 256 bytes, otherwise its longest
//! > character-boundary-safe prefix within that ceiling followed by `... [<len> bytes]` —
//! > so the text a caller reads is a function of the value it sent and the resource kind
//! > it asked for, and never of which layer refused the request.
//!
//! Two layers word that one sentence: the validation layer in front of the service, whose
//! [`Violation`](aws_smithy_schema_server::constraints::Violation) value is already
//! through the ceiling (`vendor/smithy-schema-server/src/constraints.rs`), and the handler,
//! which holds the value exactly as sent. The rows below therefore cross BOTH thresholds
//! that decide the rendering: the ceiling itself (256 bytes) and the member's own modelled
//! maximum length, which is where the answering layer changes
//! (`QueryForecast$ForecastArn` targets `Arn`, length 0..256; the longer
//! `QueryWhatIfForecast$WhatIfForecastArn` targets `LongArn`, length 0..300, both with
//! pattern `^arn:([a-z\d-]+):forecast:.*:.*:.+$` —
//! `smithy_query(query="closure",operation="QueryForecast,QueryWhatIfForecast",
//! model_path="services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json")`).
//!
//! The rows differ along what the rule generalises over: which operation is called, the
//! LENGTH of the value in bytes (a sweep from under the ceiling to past the member's
//! maximum), the FORM of the value (an ARN naming another resource kind, one carrying the
//! wrong number of name segments, one outside the modelled pattern altogether), and its
//! ENCODING (an ASCII value, and multi-byte values whose byte length passes the ceiling
//! while their character count stays inside the modelled bound — including one whose cut
//! lands mid-character, and one past the modelled bound, where the validation layer makes
//! that same cut). Negative controls keep the rule from swallowing its neighbours:
//! a value at or under the ceiling is quoted back whole, and a readable ARN that names
//! nothing stored keeps the `No resource found <arn>` wording with the ARN entire.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

/// How many bytes of a caller-controlled value survive into a message: the ceiling the
/// validation layer applies to every value it echoes
/// (`vendor/smithy-schema-server/src/constraints.rs`), and the one this service's own
/// messages render a request-derived value through.
const MAX_ECHOED_VALUE: usize = 256;

/// The modelled maximum length of `QueryWhatIfForecast$WhatIfForecastArn` (target
/// `LongArn`), in characters.
const LONG_ARN_MAX_LENGTH: usize = 300;

/// The modelled maximum length of `QueryForecast$ForecastArn` (target `Arn`), in
/// characters.
const ARN_MAX_LENGTH: usize = 256;

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // how a rejection is worded, so it turns verification off rather than signing each
    // request.
    router.set_request_authentication(false);
    router.register_service(forecastquery::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// One awsJson1_1 call: `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonForecastRuntime.{operation}"))
        // The routing key: the SigV4 credential scope's signing name (`forecast`).
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAEXAMPLEEXAMPLE/20150101/us-east-1/forecast/aws4_request, \
             SignedHeaders=content-type;host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize request body"),
        ))
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `Message` of a failure response, whatever casing the envelope used.
fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ONE rendering of `sent` a message of this service may quote back: the value itself
/// when it fits the ceiling, otherwise the longest prefix within the ceiling that ends on
/// a character boundary, the elision, and the value's own byte length.
fn expected_echo(sent: &str) -> String {
    if sent.len() <= MAX_ECHOED_VALUE {
        return sent.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while !sent.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &sent[..end], sent.len())
}

/// An ARN of the shape the control plane mints.
fn arn(resource: &str) -> String {
    engine::emulator_arn("forecast", "us-east-1", resource)
}

/// An ASCII ARN of exactly `len` characters, built from `resource`'s own prefix and padded
/// (or cut) in its trailing name so a row varies the LENGTH and nothing else.
fn sized_arn(resource: &str, len: usize) -> String {
    let padded = arn(&format!("{resource}{}", "x".repeat(len + 8)));
    assert!(
        padded.len() >= len,
        "test setup: {resource:?} padded to {} bytes cannot be cut to {len}",
        padded.len()
    );
    padded[..len].to_string()
}

/// An ARN whose trailing name is `count` copies of a two-byte character, so its BYTE
/// length can pass the echo ceiling while its CHARACTER count stays inside the member's
/// modelled bound.
fn multibyte_arn(resource: &str, count: usize) -> String {
    arn(&format!("{resource}{}", "é".repeat(count)))
}

/// One row of the class: a value this service cannot read as an ARN of the kind the
/// operation queries, and the operation it is sent to.
struct Row {
    case: String,
    operation: &'static str,
    member: &'static str,
    resource_type: &'static str,
    sent: String,
}

fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // A length SWEEP on the member with the longer bound, all of one form (an ARN naming a
    // plain forecast where a what-if forecast is queried). It crosses the echo ceiling
    // (256) and the member's modelled maximum (300) — the length at which the answering
    // layer changes from the handler to the validation layer in front of it — so the one
    // formula is pinned on both sides of both thresholds.
    for len in [
        64,
        MAX_ECHOED_VALUE - 1,
        MAX_ECHOED_VALUE,
        MAX_ECHOED_VALUE + 1,
        MAX_ECHOED_VALUE + 2,
        LONG_ARN_MAX_LENGTH - 1,
        LONG_ARN_MAX_LENGTH,
        LONG_ARN_MAX_LENGTH + 1,
        LONG_ARN_MAX_LENGTH + 2,
    ] {
        rows.push(Row {
            case: format!("a plain forecast ARN of {len} characters"),
            operation: "QueryWhatIfForecast",
            member: "WhatIfForecastArn",
            resource_type: "what-if-forecast",
            sent: sized_arn("forecast/", len),
        });
    }

    // The same length class in other FORMS, each past the ceiling and inside the member's
    // bound, so the rendering cannot be keyed to one shape of value.
    for (case, resource) in [
        (
            "a what-if forecast ARN missing its analysis name",
            "what-if-forecast/",
        ),
        (
            "a what-if forecast ARN with a name segment too many",
            "what-if-forecast/electricity_analysis/price_cut/",
        ),
        ("a what-if analysis ARN", "what-if-analysis/"),
        ("a dataset ARN", "dataset/"),
    ] {
        rows.push(Row {
            case: format!("{case}, at the member's maximum length"),
            operation: "QueryWhatIfForecast",
            member: "WhatIfForecastArn",
            resource_type: "what-if-forecast",
            sent: sized_arn(resource, LONG_ARN_MAX_LENGTH),
        });
    }

    // Outside the modelled pattern at the same length: refused by the validation layer
    // rather than the handler, and so the proof that the rendering does not depend on
    // which layer answered — this row and the sweep's 300-character row are the same
    // length and must report the same byte count after the same ceiling.
    rows.push(Row {
        case: format!("a URL rather than an ARN, of {LONG_ARN_MAX_LENGTH} characters"),
        operation: "QueryWhatIfForecast",
        member: "WhatIfForecastArn",
        resource_type: "what-if-forecast",
        sent: format!(
            "https://forecast.us-east-1.amazonaws.com/{}",
            "x".repeat(LONG_ARN_MAX_LENGTH)
        )[..LONG_ARN_MAX_LENGTH]
            .to_string(),
    });

    // Multi-byte values: the ceiling counts BYTES while the modelled bound counts
    // characters, so a value well inside its member's length can still be past the
    // ceiling — on either operation. `é` is two bytes, so an odd number of bytes into the
    // run puts the cut mid-character and the prefix must end before it.
    for (case, count) in [
        ("whose cut lands mid-character", 120),
        ("whose cut lands on a character boundary", 121),
    ] {
        rows.push(Row {
            case: format!("a multi-byte plain forecast ARN {case}"),
            operation: "QueryWhatIfForecast",
            member: "WhatIfForecastArn",
            resource_type: "what-if-forecast",
            sent: multibyte_arn("forecast/", count),
        });
    }
    // A multi-byte value past the member's modelled maximum in CHARACTERS: refused by the
    // validation layer, so this is the row that shows its cut agrees with the handler
    // path's on a value whose ceiling lands mid-character.
    rows.push(Row {
        case: "a multi-byte plain forecast ARN past the member's maximum length".to_string(),
        operation: "QueryWhatIfForecast",
        member: "WhatIfForecastArn",
        resource_type: "what-if-forecast",
        sent: multibyte_arn("forecast/", LONG_ARN_MAX_LENGTH),
    });
    rows.push(Row {
        case: "a multi-byte what-if forecast ARN, past the ceiling in bytes and inside \
               `Arn`'s bound in characters"
            .to_string(),
        operation: "QueryForecast",
        member: "ForecastArn",
        resource_type: "forecast",
        sent: multibyte_arn("what-if-forecast/electricity_analysis/", 110),
    });

    // The shorter member's own thresholds, so the rule is pinned on both operations: at
    // the ceiling the value is quoted whole, and one past the member's maximum it is the
    // validation layer that elides it.
    for len in [MAX_ECHOED_VALUE, ARN_MAX_LENGTH + 1, ARN_MAX_LENGTH + 2] {
        rows.push(Row {
            case: format!("a what-if forecast ARN of {len} characters"),
            operation: "QueryForecast",
            member: "ForecastArn",
            resource_type: "forecast",
            sent: sized_arn("what-if-forecast/electricity_analysis/", len),
        });
    }

    rows
}

/// The whole class: an unreadable ARN is quoted back through ONE echo ceiling, whether the
/// handler or the validation layer in front of it refused the request.
#[tokio::test]
async fn an_unreadable_arn_is_quoted_back_through_one_echo_ceiling() {
    let (_engine, router) = harness();

    for row in rows() {
        let Row {
            case,
            operation,
            member,
            resource_type,
            sent,
        } = row;
        let case = format!("{operation} / {case} / {} bytes", sent.len());

        let (status, response) = call(
            &router,
            operation,
            &json!({member: &sent, "Filters": {"item_id": "client_21"}}),
        )
        .await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {response}"
        );
        let actual = message(&response);
        let expected = format!(
            "Invalid ARN for {resource_type}: {}",
            expected_echo(&sent)
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // The point of the ceiling: a value a caller controls cannot stretch the answer.
        // The echoed part is at most the ceiling plus the elision it is marked with.
        let echoed = actual
            .strip_prefix(&format!("Invalid ARN for {resource_type}: "))
            .unwrap_or_default();
        assert!(
            echoed.len() <= MAX_ECHOED_VALUE + "... [999999 bytes]".len(),
            "case {case:?}: the echoed value must stay within the ceiling, got {} bytes: \
             {echoed:?}",
            echoed.len()
        );
    }
}

/// The neighbours of the rule, so it cannot swallow them: a value within the ceiling is
/// quoted back entire, and a readable ARN of the queried kind that names nothing stored
/// keeps the `No resource found <arn>` wording with the whole ARN in it — that message is
/// worded by one layer only and reports the ARN as sent.
#[tokio::test]
async fn the_ceiling_leaves_a_short_value_and_the_not_found_wording_alone() {
    let (_engine, router) = harness();

    // Within the ceiling: nothing is elided, whatever the form.
    for (case, operation, member, resource_type, sent) in [
        (
            "a plain forecast ARN",
            "QueryWhatIfForecast",
            "WhatIfForecastArn",
            "what-if-forecast",
            arn("forecast/electricity_demand"),
        ),
        (
            "a what-if forecast ARN",
            "QueryForecast",
            "ForecastArn",
            "forecast",
            arn("what-if-forecast/electricity_analysis/price_cut"),
        ),
    ] {
        let (status, response) = call(
            &router,
            operation,
            &json!({member: &sent, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        let case = format!("{operation} / {case} / {} bytes", sent.len());
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        let actual = message(&response);
        assert_eq!(
            actual,
            format!("Invalid ARN for {resource_type}: {sent}"),
            "case {case:?}: a value within the ceiling is quoted back whole, got {actual:?}"
        );
    }

    // A readable what-if forecast ARN at the member's modelled maximum names a resource
    // the store does not hold: the absent-resource wording, with the ARN entire.
    let absent = sized_arn("what-if-forecast/electricity_analysis/", LONG_ARN_MAX_LENGTH);
    assert!(
        absent.len() > MAX_ECHOED_VALUE,
        "test setup: the absent ARN must be longer than the ceiling, got {} bytes",
        absent.len()
    );
    let (status, response) = call(
        &router,
        "QueryWhatIfForecast",
        &json!({"WhatIfForecastArn": &absent, "Filters": {"item_id": "client_21"}}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"an absent what-if forecast at the member's maximum length\": expected HTTP \
         404, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case \"an absent what-if forecast at the member's maximum length\": expected \
         ResourceNotFoundException, got {response}"
    );
    let actual = message(&response);
    assert_eq!(
        actual,
        format!("No resource found {absent}"),
        "case \"an absent what-if forecast at the member's maximum length\": expected the \
         absent-resource wording with the ARN as sent, got {actual:?}"
    );
}
