// SPDX-License-Identifier: CC0-1.0

//! `TestEventPattern` as the PRE-FLIGHT a client runs before it depends on a
//! rule, end to end over the wire (`awsJson1_1`, `POST /` with `X-Amz-Target:
//! AWSEvents.<Operation>`) through the real `MultiServiceRouter`, with SQS
//! mounted on the same engine so the routing verdict is read where a client
//! reads it: on the target queue.
//!
//! The RULE asserted here: `TestEventPattern` answers HTTP 200 with `Result`
//! true exactly when the supplied event matches the supplied pattern under the
//! event-pattern language, and that verdict is the SAME one routing applies — the
//! same pattern on an enabled rule delivers the event to the rule's targets when
//! and only when `Result` is true. So a pattern validated with this operation
//! predicts the delivery the client is about to rely on, for every row below.
//!
//! Each row of the table narrows an event on a field of the ENVELOPE and a field
//! inside `detail`, and the rows differ along what decides the verdict: the
//! nested value, how many values a term lists, which field of the conjunction
//! fails, the type compared (string, number), the depth of the field path, and
//! whether the field is present at all. The envelope members the rows build come
//! from `TestEventPattern$Event` ("the following fields are mandatory: id account
//! source time region resources detail-type").

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-east-2";

/// The event every row below pre-flights and then publishes: one state change of
/// a named pipeline run.
const SOURCE: &str = "com.demo.pipelines";
const DETAIL_TYPE: &str = "Pipeline Run State Change";
const PIPELINE: &str = "nightly-aggregate";

/// EventBridge and SQS on one engine: the rule's target is a queue, because a
/// delivery is read back there.
struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router }
}

/// One `awsJson` RPC call against the service whose signing name is
/// `signing_name` — the credential scope is what routes it.
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
async fn given_queue(emulator: &Emulator, case: &str, name: &str) -> (String, String) {
    let (status, body) = sqs(emulator, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateQueue to answer 200, got {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {body}"))
        .to_string();
    let (status, body) = sqs(
        emulator,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetQueueAttributes to answer 200, got {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a QueueArn, got {body}"))
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
        "case {case:?}: expected SetQueueAttributes to answer 200, got {status} {body}"
    );
    (url, arn)
}

/// How many messages are waiting on the queue at `url`.
async fn delivered(emulator: &Emulator, case: &str, url: &str) -> usize {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ReceiveMessage to answer 200, got {status} {body}"
    );
    body["Messages"].as_array().map_or(0, Vec::len)
}

/// The envelope a client pre-flights: every field `TestEventPattern$Event` makes
/// mandatory, carrying `detail`.
fn envelope(detail: &Value) -> Value {
    json!({
        "version": "0",
        "id": "0f1b4a8c-2b55-4a1f-9d0e-5f2c7f0a9c31",
        "account": engine::emulator_account_id(),
        "source": SOURCE,
        "time": "2026-02-03T08:15:00Z",
        "region": REGION,
        "resources": [],
        "detail-type": DETAIL_TYPE,
        "detail": detail,
    })
}

/// The `detail` of the run every positive row describes.
fn base_detail() -> Value {
    json!({
        "pipelineName": PIPELINE,
        "state": "SUCCEEDED",
        "attempt": 2,
        "trigger": { "kind": "schedule", "cron": "0 3 * * ? *" },
    })
}

/// The verdict `TestEventPattern` answers for one (pattern, event) pair.
async fn preflight(emulator: &Emulator, case: &str, pattern: &Value, event: &Value) -> bool {
    let (status, body) = events(
        emulator,
        "TestEventPattern",
        &json!({ "EventPattern": pattern.to_string(), "Event": event.to_string() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected TestEventPattern to answer 200, got {status} {body}"
    );
    body["Result"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: expected a boolean Result, got {body}"))
}

/// `Result` is the event-pattern verdict, and the same pattern on a rule delivers
/// exactly when that verdict is true.
#[tokio::test]
async fn test_event_pattern_predicts_the_delivery_a_rule_with_that_pattern_makes() {
    // (case, the pattern, the event's detail, whether the pattern matches it)
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "an envelope conjunction narrowed to one nested name",
            json!({
                "source": [SOURCE],
                "detail-type": [DETAIL_TYPE],
                "detail": { "pipelineName": [PIPELINE] },
            }),
            base_detail(),
            true,
        ),
        (
            "the same pattern against another name under that nested field",
            json!({
                "source": [SOURCE],
                "detail-type": [DETAIL_TYPE],
                "detail": { "pipelineName": [PIPELINE] },
            }),
            json!({ "pipelineName": "hourly-rollup", "state": "SUCCEEDED" }),
            false,
        ),
        (
            "a nested term listing several names, one of them the event's",
            json!({
                "source": [SOURCE],
                "detail": { "pipelineName": ["hourly-rollup", PIPELINE, "weekly-compact"] },
            }),
            base_detail(),
            true,
        ),
        (
            "the envelope's source is not the one the pattern names",
            json!({
                "source": ["com.demo.other"],
                "detail": { "pipelineName": [PIPELINE] },
            }),
            base_detail(),
            false,
        ),
        (
            "the envelope's detail-type is not the one the pattern names",
            json!({
                "source": [SOURCE],
                "detail-type": ["Pipeline Run Scheduled"],
                "detail": { "pipelineName": [PIPELINE] },
            }),
            base_detail(),
            false,
        ),
        (
            "two nested fields, both holding",
            json!({
                "source": [SOURCE],
                "detail": { "pipelineName": [PIPELINE], "state": ["FAILED", "SUCCEEDED"] },
            }),
            base_detail(),
            true,
        ),
        (
            "two nested fields, the second one failing",
            json!({
                "source": [SOURCE],
                "detail": { "pipelineName": [PIPELINE], "state": ["FAILED"] },
            }),
            base_detail(),
            false,
        ),
        (
            "a number compared under detail",
            json!({
                "source": [SOURCE],
                "detail": { "attempt": [{ "numeric": [">=", 2] }] },
            }),
            base_detail(),
            true,
        ),
        (
            "a number outside the compared range",
            json!({
                "source": [SOURCE],
                "detail": { "attempt": [{ "numeric": ["<", 2] }] },
            }),
            base_detail(),
            false,
        ),
        (
            "a field two levels inside detail",
            json!({
                "source": [SOURCE],
                "detail": { "trigger": { "kind": ["schedule"] } },
            }),
            base_detail(),
            true,
        ),
        (
            "a prefix of the nested name",
            json!({
                "source": [SOURCE],
                "detail": { "pipelineName": [{ "prefix": "nightly-" }] },
            }),
            base_detail(),
            true,
        ),
        (
            "the nested field the pattern names is absent from the event",
            json!({
                "source": [SOURCE],
                "detail": { "pipelineName": [PIPELINE] },
            }),
            json!({ "state": "SUCCEEDED" }),
            false,
        ),
    ];

    for (index, (case, pattern, detail, expected)) in rows.into_iter().enumerate() {
        let emulator = emulator();

        // 1. The pre-flight: what the client asks before it builds the rule.
        let verdict = preflight(&emulator, case, &pattern, &envelope(&detail)).await;
        assert_eq!(
            verdict, expected,
            "case {case:?}: expected Result {expected}, got {verdict}"
        );

        // 2. The delivery that verdict predicts, through the same pattern on a
        //    rule and the same event through `PutEvents`.
        let name = format!("preflight-{index}");
        let (url, arn) = given_queue(&emulator, case, &name).await;
        let (status, body) = events(
            &emulator,
            "PutRule",
            &json!({
                "Name": name,
                "EventPattern": pattern.to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRule to answer 200, got {status} {body}"
        );
        let (status, body) = events(
            &emulator,
            "PutTargets",
            &json!({ "Rule": name, "Targets": [{ "Id": "t1", "Arn": arn }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutTargets to answer 200, got {status} {body}"
        );
        let (status, body) = events(
            &emulator,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": SOURCE,
                    "DetailType": DETAIL_TYPE,
                    "Detail": detail.to_string(),
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutEvents to answer 200, got {status} {body}"
        );
        assert_eq!(
            body["FailedEntryCount"],
            json!(0),
            "case {case:?}: expected no failed entry, got {body}"
        );

        let count = delivered(&emulator, case, &url).await;
        let wanted = usize::from(expected);
        assert_eq!(
            count, wanted,
            "case {case:?}: TestEventPattern answered {verdict}, so expected {wanted} \
             delivery/deliveries to the rule's target, got {count}"
        );
    }
}
