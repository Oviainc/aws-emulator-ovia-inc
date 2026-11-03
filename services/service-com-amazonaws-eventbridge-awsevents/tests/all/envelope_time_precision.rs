// SPDX-License-Identifier: CC0-1.0

//! The `time` field of the event envelope a target receives, end to end over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter`, with SQS mounted on the same engine so the envelope is read
//! where a consumer reads it: off the queue a rule's target names.
//!
//! The RULE asserted here: the envelope's `time` is an ISO-8601 instant at WHOLE-SECOND
//! precision closed by `Z` — `%Y-%m-%dT%H:%M:%SZ`, twenty characters — and it names the
//! second of the event's own instant, which is the entry's `Time` when it carries one
//! and the ingestion instant when it does not. A finer reading is TRUNCATED to that
//! second rather than rounded, and never carries a fractional part: the field's form is
//! what every consumer parses `time` with, so an envelope that carries more precision
//! than AWS emits is one no consumer can read.
//!
//! The rows differ along what the rule generalises over: whether the entry names its own
//! `Time` at all, and what sub-second remainder that instant carries — none, a small
//! one, one that would round the second UP, and one at the far end of the second — over
//! two different instants plus the epoch itself.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The source and detail type the rule below matches on.
const SOURCE: &str = "com.demo.clocks";
const DETAIL_TYPE: &str = "Tick Recorded";
const DETAIL: &str = r#"{"tick":1}"#;

/// `2026-01-01T00:00:00Z` in epoch seconds, which is the instant the rows that name
/// their own `Time` are written around.
const NEW_YEAR: f64 = 1_767_225_600.0;

/// How many characters the format carries: `2026-01-01T00:00:00Z`.
const INSTANT_LENGTH: usize = 20;

/// EventBridge and SQS on one engine, because the envelope is asserted on the queue a
/// rule's target names.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    Emulator { router }
}

/// One `awsJson` RPC call against the service whose signing name is `signing_name` —
/// the credential scope is what routes it.
async fn rpc(
    emulator: &Emulator,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/{signing_name}/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!("AWS4-HMAC-SHA256 Credential={credential}"),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

async fn events(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "events", "AWSEvents", "1.1", operation, body).await
}

async fn sqs(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    rpc(emulator, "sqs", "AmazonSQS", "1.0", operation, body).await
}

/// A queue called `name`, carrying the policy a rule's target needs: EventBridge
/// "relies on resource-based policies" for a queue (`PutTargets`), so one that has
/// granted the service nothing receives nothing — the rule
/// `tests/all/queue_target_policy_authorization.rs` is the suite for. Answers its url
/// and the ARN a rule names it by.
async fn given_queue(emulator: &Emulator, name: &str) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {name}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "Policy": crate::target_dispatch::granting_events(&arn) },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: SetQueueAttributes answered {status} {body}"
    );
    (url, arn)
}

/// The one message body waiting on the queue at `url`, as raw text.
async fn received(emulator: &Emulator, url: &str, case: &str) -> String {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {body}"
    );
    let messages = body["Messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        messages.len(),
        1,
        "case {case:?}: expected one delivered envelope on the queue, got {messages:?}"
    );
    messages[0]["Body"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the message carries no Body: {body}"))
        .to_string()
}

/// Whether `text` is an instant written the way the envelope's `time` is:
/// `%Y-%m-%dT%H:%M:%SZ`, with no fractional part and no zone but `Z`.
fn is_whole_second_instant(text: &str) -> bool {
    if text.len() != INSTANT_LENGTH {
        return false;
    }
    text.bytes().enumerate().all(|(index, byte)| match index {
        4 | 7 => byte == b'-',
        10 => byte == b'T',
        13 | 16 => byte == b':',
        19 => byte == b'Z',
        _ => byte.is_ascii_digit(),
    })
}

/// One row: what a failing assertion calls it, the `Time` the entry names (`None` for an
/// entry that names none), and the instant the envelope must carry (`None` where the
/// instant is the server clock's and only the FORM is a requirement).
type Row = (&'static str, Option<f64>, Option<&'static str>);

/// The envelope's `time` is a whole-second instant, whatever precision the event's own
/// instant carries.
#[tokio::test]
async fn the_delivered_envelope_carries_a_whole_second_instant() {
    let emulator = emulator();
    let (url, queue_arn) = given_queue(&emulator, "envelope-time").await;
    let (status, body) = events(
        &emulator,
        "PutRule",
        &json!({
            "Name": "envelope-time",
            "EventPattern": json!({ "source": [SOURCE] }).to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let (status, body) = events(
        &emulator,
        "PutTargets",
        &json!({
            "Rule": "envelope-time",
            "Targets": [{ "Id": "queue", "Arn": queue_arn }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");

    let rows: [Row; 7] = [
        (
            "an entry that names no Time, so the event is timestamped as it is ingested",
            None,
            None,
        ),
        (
            "an entry whose Time is exactly on a second",
            Some(NEW_YEAR),
            Some("2026-01-01T00:00:00Z"),
        ),
        (
            "an entry whose Time carries a fractional second",
            Some(NEW_YEAR + 0.725),
            Some("2026-01-01T00:00:00Z"),
        ),
        (
            // Truncated, not rounded: the field names the second the instant is IN.
            "an entry whose Time is at the far end of its second",
            Some(NEW_YEAR + 0.999_999),
            Some("2026-01-01T00:00:00Z"),
        ),
        (
            "an entry whose Time is minutes into the day",
            Some(NEW_YEAR + 123.0),
            Some("2026-01-01T00:02:03Z"),
        ),
        (
            "the same instant carrying half a second",
            Some(NEW_YEAR + 123.5),
            Some("2026-01-01T00:02:03Z"),
        ),
        (
            "the epoch itself",
            Some(0.0),
            Some("1970-01-01T00:00:00Z"),
        ),
    ];

    for (case, at, expected) in rows {
        let mut entry = json!({ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": DETAIL });
        if let Some(at) = at {
            entry["Time"] = json!(at);
        }
        let (status, body) = events(&emulator, "PutEvents", &json!({ "Entries": [entry] })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutEvents answered {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected the entry to be accepted, got {body}"
        );

        let envelope: Value = {
            let text = received(&emulator, &url, case).await;
            serde_json::from_str(&text).unwrap_or_else(|error| {
                panic!("case {case:?}: the delivered body is not JSON ({error}): {text}")
            })
        };
        let time = envelope["time"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the envelope carries no time: {envelope}"))
            .to_string();
        assert!(
            is_whole_second_instant(&time),
            "case {case:?}: expected a whole-second instant closed by Z, got {time:?}"
        );
        if let Some(expected) = expected {
            assert_eq!(
                time, expected,
                "case {case:?}: expected {expected:?}, got {time:?}"
            );
        }
    }
}
