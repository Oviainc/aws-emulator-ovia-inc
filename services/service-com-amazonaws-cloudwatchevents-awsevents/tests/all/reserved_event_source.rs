// SPDX-License-Identifier: CC0-1.0

//! `PutEvents` and the namespace Amazon Web Services reserves for its own
//! services' events, on this legacy model of the EventBridge API.
//!
//! The RULE asserted here: an entry whose `Source` is in the reserved `aws.`
//! namespace is refused through the per-entry failure channel — the CALL still
//! answers 200, that entry's result carries an `ErrorCode` and no `EventId`,
//! `FailedEntryCount` counts it, it is never recorded on the bus, and every
//! other entry of the same request is published as usual.
//!
//! The table spans the namespace a source claims rather than one recorded
//! value: the rows differ along the AWS service a forged source names, the
//! partner form, a bare prefix, and sources that merely contain `aws` without
//! being in the reserved namespace.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchevents_awsevents::state::ComAmazonawsCloudwatcheventsAwseventsState;

use crate::support::{emulator, Emulator, DEFAULT_BUS};

/// The per-entry error code this API publishes for a `Source` the caller may
/// not publish under: "You do not have permissions to publish events with this
/// source onto this event bus" (`PutEventsResultEntry$ErrorCode` of the
/// EventBridge model this legacy model is an alias of). Spelled out here rather
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

/// The table both tests below span. `aws.cognito-idp` is deliberately absent:
/// the rule is about the namespace, not about one service's label.
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

impl Emulator {
    /// The `(EventId, Source)` of every event the bus's log holds, in the order
    /// it recorded them — read out of the store this service's handlers wrote.
    fn recorded_sources(&self, bus: &str) -> Vec<(String, String)> {
        let state = self
            .engine
            .state::<ComAmazonawsCloudwatcheventsAwseventsState>();
        let store = state.lock();
        store
            .bus(bus)
            .into_iter()
            .flat_map(|bus| bus.events.iter())
            .map(|event| {
                (
                    event.event_id.clone(),
                    event.source.clone().unwrap_or_default(),
                )
            })
            .collect()
    }
}

/// One entry per row, each in its OWN call, so the verdict on a source does not
/// depend on what else the batch carried.
#[tokio::test]
async fn put_events_refuses_a_source_in_the_reserved_aws_namespace() {
    for case in source_cases() {
        let Case { label, source, reserved } = case;
        let emulator = emulator();
        let answer = emulator
            .call(
                "PutEvents",
                json!({
                    "Entries": [{
                        "Source": source,
                        "DetailType": "Order Created",
                        "Detail": r#"{"amount":1}"#,
                    }],
                }),
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?} source {source:?}: PutEvents answered {} {:?}",
            answer.status, answer.body
        );
        let expected_failures = i64::from(reserved);
        assert_eq!(
            answer.body["FailedEntryCount"].as_i64(),
            Some(expected_failures),
            "case {label:?} source {source:?}: expected FailedEntryCount {expected_failures}, \
             got {:?}",
            answer.body
        );

        let result = &answer.body["Entries"][0];
        let code = result.get("ErrorCode").and_then(Value::as_str);
        let event_id = result.get("EventId").and_then(Value::as_str);
        let recorded = emulator.recorded_sources(DEFAULT_BUS);
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
                recorded.is_empty(),
                "case {label:?} source {source:?}: a refused entry is never recorded, \
                 got {recorded:?}"
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
                recorded,
                vec![(
                    event_id.unwrap_or_default().to_string(),
                    source.to_string()
                )],
                "case {label:?} source {source:?}: expected the accepted event recorded on \
                 the bus, got {recorded:?}"
            );
        }
    }
}

/// The reserved-namespace rule is an ENTRY-level one: one call carrying both
/// reserved and ordinary sources answers 200, counts exactly the reserved
/// entries, keeps one result per entry at the request's own index, and records
/// every other entry. The batch is sent in the table's order and reversed, so
/// no verdict can depend on an entry's position.
#[tokio::test]
async fn a_reserved_entry_fails_alone_inside_a_mixed_batch() {
    for (order, cases) in [
        ("as listed", source_cases()),
        ("reversed", source_cases().into_iter().rev().collect::<Vec<_>>()),
    ] {
        let emulator = emulator();
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
        let answer = emulator
            .call("PutEvents", json!({ "Entries": entries }))
            .await;
        assert_eq!(
            answer.status, 200,
            "order {order:?}: PutEvents answered {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["FailedEntryCount"].as_u64(),
            Some(expected_failures as u64),
            "order {order:?}: expected FailedEntryCount {expected_failures}, got {:?}",
            answer.body
        );

        let results = answer.body["Entries"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            cases.len(),
            "order {order:?}: expected one result per entry ({}), got {:?}",
            cases.len(),
            answer.body
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

        let recorded = emulator.recorded_sources(DEFAULT_BUS);
        assert_eq!(
            recorded, published,
            "order {order:?}: expected exactly the accepted entries {published:?} recorded, \
             got {recorded:?}"
        );
    }
}
