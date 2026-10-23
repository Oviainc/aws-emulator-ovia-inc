// SPDX-License-Identifier: CC0-1.0

//! `PutEvents` and the namespace Amazon Web Services reserves for its own
//! services' events, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real
//! `MultiServiceRouter`.
//!
//! The RULE asserted here: an entry whose `Source` is in the reserved `aws.`
//! namespace is refused through the per-entry failure channel — the CALL still
//! answers 200, that entry's result carries an `ErrorCode` and no `EventId`,
//! `FailedEntryCount` counts it, it reaches no rule's targets, and every other
//! entry of the same request is published as usual.
//!
//! Each test spans a TABLE of sources rather than one recorded value: the rows
//! differ along the namespace the source claims, where the reserved prefix sits
//! in it, how long the label after the prefix is, and — for the batch case —
//! the position a reserved entry takes among accepted ones.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::ComAmazonawsEventbridgeAwseventsState;
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The per-entry error code this API publishes for a `Source` the caller may
/// not publish under: "You do not have permissions to publish events with this
/// source onto this event bus" — `PutEventsResultEntry$ErrorCode`'s own list of
/// non-retryable codes (`smithy_query(query="doc",
/// shape="PutEventsResultEntry", member="ErrorCode")`). Spelled out here rather
/// than imported, so a wrong code in the implementation cannot make this pass.
const NOT_AUTHORIZED_FOR_SOURCE: &str = "NotAuthorizedForSourceException";

/// The message that accompanies that code. It is a CONSTANT of the condition —
/// the verdict is on the namespace the source claims — so it is the same text
/// for every reserved row and names nothing from the request.
const NOT_AUTHORIZED_FOR_SOURCE_MESSAGE: &str = "Not authorized for the source.";

/// One row of the source table: a label for the assertion messages, the
/// `Source` to publish under, and whether that source lies in the reserved
/// namespace.
struct Case {
    label: &'static str,
    source: &'static str,
    reserved: bool,
}

/// The table every test below spans. Reserved rows differ along the AWS service
/// the forged source names, the partner form (`aws.partner/...`, which only
/// `PutPartnerEvents` may write), and a bare prefix with nothing after it;
/// accepted rows differ along where `aws` appears in a source that is NOT in
/// the reserved namespace — as a longer first label, inside the name, and
/// absent altogether. `aws.cognito-idp` is deliberately NOT among them: the
/// rule is about the namespace, not about one service's label.
fn source_cases() -> Vec<Case> {
    vec![
        Case { label: "custom reverse-dns source", source: "com.demo.widgets", reserved: false },
        Case { label: "reserved: object storage", source: "aws.s3", reserved: true },
        Case { label: "custom source whose first label merely starts with aws", source: "awsome.example", reserved: false },
        Case { label: "reserved: audit trail", source: "aws.cloudtrail", reserved: true },
        Case { label: "custom source carrying aws. inside it", source: "com.aws.example", reserved: false },
        Case { label: "reserved: partner event source form", source: "aws.partner/example.com/1234/stream", reserved: true },
        Case { label: "custom source with no aws at all", source: "my-app.orders", reserved: false },
        Case { label: "reserved: the bare prefix", source: "aws.", reserved: true },
        Case { label: "reserved: an invented service label", source: "aws.not-a-real-service", reserved: true },
    ]
}

/// The emulator: one router carrying only this service over one shared engine,
/// plus that engine, so a test can read the routing log the handler wrote.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Emulator { router, engine }
}

async fn call(emulator: &Emulator, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, SignedHeaders=host, Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("authorization", format!("AWS4-HMAC-SHA256 Credential={credential}"))
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

/// The routing log `PutEvents` appended to, as `(EventId, source, target ARNs)`
/// per accepted event — so what a rule's targets would have received is read
/// from state rather than inferred from the response.
fn delivered(emulator: &Emulator) -> Vec<(String, String, Vec<String>)> {
    emulator
        .engine
        .state::<ComAmazonawsEventbridgeAwseventsState>()
        .lock()
        .delivered
        .iter()
        .map(|event| {
            (
                event.event_id.clone(),
                event.source.clone().unwrap_or_default(),
                event.target_arns.clone(),
            )
        })
        .collect()
}

/// The ARN of the queue the catch-all rule below delivers to.
fn queue_arn() -> String {
    format!(
        "arn:aws:sqs:{REGION}:{}:reserved-source-probe",
        engine::emulator_account_id()
    )
}

/// A rule on the default bus that matches EVERY event carrying a source, with
/// one SQS target: the positive control, so an accepted entry demonstrably
/// reaches a target and a refused one demonstrably does not.
async fn catch_all_rule(emulator: &Emulator) {
    let (status, body) = call(
        emulator,
        "PutRule",
        &json!({ "Name": "catch-all", "EventPattern": r#"{"source":[{"exists":true}]}"# }),
    )
    .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let (status, body) = call(
        emulator,
        "PutTargets",
        &json!({
            "Rule": "catch-all",
            "Targets": [{ "Id": "queue", "Arn": queue_arn() }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");
}

/// One entry per row, each in its OWN call, so the verdict on a source does not
/// depend on what else the batch carried: a reserved source is refused with
/// `NotAuthorizedForSourceException`, no `EventId` and `FailedEntryCount` 1, and
/// it reaches no target; any other source is published and routed.
#[tokio::test]
async fn put_events_refuses_a_source_in_the_reserved_aws_namespace() {
    for case in source_cases() {
        let Case { label, source, reserved } = case;
        let emulator = emulator();
        catch_all_rule(&emulator).await;

        let (status, body) = call(
            &emulator,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": source,
                    "DetailType": "Order Created",
                    "Detail": r#"{"amount":1}"#,
                }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?} source {source:?}: PutEvents answered {status}, body {body}"
        );
        let expected_failures = i64::from(reserved);
        assert_eq!(
            body["FailedEntryCount"].as_i64(),
            Some(expected_failures),
            "case {label:?} source {source:?}: expected FailedEntryCount {expected_failures}, \
             got {body}"
        );

        let result = &body["Entries"][0];
        let code = result.get("ErrorCode").and_then(Value::as_str);
        let event_id = result.get("EventId").and_then(Value::as_str);
        let log = delivered(&emulator);
        if reserved {
            assert_eq!(
                code,
                Some(NOT_AUTHORIZED_FOR_SOURCE),
                "case {label:?} source {source:?}: expected ErrorCode \
                 {NOT_AUTHORIZED_FOR_SOURCE}, got {result}"
            );
            assert_eq!(
                event_id, None,
                "case {label:?} source {source:?}: a refused entry carries no EventId, \
                 got {result}"
            );
            assert_eq!(
                result.get("ErrorMessage").and_then(Value::as_str),
                Some(NOT_AUTHORIZED_FOR_SOURCE_MESSAGE),
                "case {label:?} source {source:?}: expected ErrorMessage \
                 {NOT_AUTHORIZED_FOR_SOURCE_MESSAGE:?}, got {result}"
            );
            assert!(
                log.is_empty(),
                "case {label:?} source {source:?}: a refused entry reaches no target, \
                 got {log:?}"
            );
        } else {
            assert_eq!(
                code, None,
                "case {label:?} source {source:?}: expected no ErrorCode, got {result}"
            );
            assert!(
                event_id.is_some_and(|id| !id.is_empty()),
                "case {label:?} source {source:?}: an accepted entry carries an EventId, \
                 got {result}"
            );
            assert_eq!(
                log,
                vec![(
                    event_id.unwrap_or_default().to_string(),
                    source.to_string(),
                    vec![queue_arn()],
                )],
                "case {label:?} source {source:?}: expected the accepted event routed to the \
                 rule's target, got {log:?}"
            );
        }
    }
}

/// The reserved-namespace rule is an ENTRY-level one: one call carrying both
/// reserved and ordinary sources answers 200, counts exactly the reserved
/// entries in `FailedEntryCount`, keeps one result per entry at the request's
/// own index, and publishes every other entry. The batch is sent twice — in the
/// table's order and reversed — so no verdict can depend on an entry's
/// position.
#[tokio::test]
async fn a_reserved_entry_fails_alone_inside_a_mixed_batch() {
    for (order, cases) in [
        ("as listed", source_cases()),
        ("reversed", source_cases().into_iter().rev().collect::<Vec<_>>()),
    ] {
        let emulator = emulator();
        catch_all_rule(&emulator).await;

        let entries: Vec<Value> = cases
            .iter()
            .map(|case| {
                json!({
                    "Source": case.source,
                    "DetailType": "Order Created",
                    "Detail": r#"{"amount":1}"#,
                })
            })
            .collect();
        let expected_failures = cases.iter().filter(|case| case.reserved).count();
        let (status, body) =
            call(&emulator, "PutEvents", &json!({ "Entries": entries })).await;
        assert_eq!(
            status, 200,
            "order {order:?}: PutEvents answered {status}, body {body}"
        );
        assert_eq!(
            body["FailedEntryCount"].as_u64(),
            Some(expected_failures as u64),
            "order {order:?}: expected FailedEntryCount {expected_failures}, got {body}"
        );

        let results = body["Entries"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            cases.len(),
            "order {order:?}: expected one result per entry ({}), got {body}",
            cases.len()
        );
        let mut published: Vec<(String, String)> = Vec::new();
        for (index, case) in cases.iter().enumerate() {
            let result = &results[index];
            let code = result.get("ErrorCode").and_then(Value::as_str);
            let expected_code = if case.reserved { Some(NOT_AUTHORIZED_FOR_SOURCE) } else { None };
            assert_eq!(
                code, expected_code,
                "order {order:?} entry {index} {label:?} source {source:?}: expected ErrorCode \
                 {expected_code:?}, got {result}",
                label = case.label,
                source = case.source,
            );
            if case.reserved {
                assert_eq!(
                    result.get("ErrorMessage").and_then(Value::as_str),
                    Some(NOT_AUTHORIZED_FOR_SOURCE_MESSAGE),
                    "order {order:?} entry {index} {label:?} source {source:?}: expected \
                     ErrorMessage {NOT_AUTHORIZED_FOR_SOURCE_MESSAGE:?}, got {result}",
                    label = case.label,
                    source = case.source,
                );
            }
            match result.get("EventId").and_then(Value::as_str) {
                Some(id) if !case.reserved && !id.is_empty() => {
                    published.push((id.to_string(), case.source.to_string()));
                }
                other => assert!(
                    case.reserved && other.is_none(),
                    "order {order:?} entry {index} {label:?} source {source:?}: expected an \
                     EventId only for an accepted entry, got {result}",
                    label = case.label,
                    source = case.source,
                ),
            }
        }

        // Only the ordinary entries reached the rule's target, each under the
        // EventId the response reported and in the request's own order.
        let routed: Vec<(String, String)> = delivered(&emulator)
            .into_iter()
            .map(|(event_id, source, targets)| {
                assert_eq!(
                    targets,
                    vec![queue_arn()],
                    "order {order:?}: event {event_id} was routed to {targets:?}"
                );
                (event_id, source)
            })
            .collect();
        assert_eq!(
            routed, published,
            "order {order:?}: expected exactly the accepted entries {published:?} routed, \
             got {routed:?}"
        );
    }
}
