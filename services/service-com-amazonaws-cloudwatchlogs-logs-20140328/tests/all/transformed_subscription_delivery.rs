// SPDX-License-Identifier: CC0-1.0

//! The TRANSFORMED reading of an ingested event on the subscription-filter delivery path,
//! end to end over the wire with the Kinesis emulator mounted beside this service, so the
//! delivered record is read where a consumer reads it.
//!
//! RULE: a subscription filter configured with `applyOnTransformedLogs` on a log group that
//! has a transformer is applied to the TRANSFORMED version of each ingested event — "If the
//! log group uses either a log-group level or account-level transformer, and you specify
//! true, the subscription filter will be applied on the transformed version of the log
//! events instead of the original ingested log events"
//! (`PutSubscriptionFilter$applyOnTransformedLogs`) — so that version is both what its
//! `filterPattern` selects on and what the delivered envelope carries, while the event the
//! log group STORED is unchanged and `GetLogEvents` still reports the ingested bytes. A
//! filter that does not ask for it, and one on a group with no transformer, carries the event
//! as ingested.
//!
//! The transformed version is what the group's processor chain produces: the vended
//! `parseVPC` processor converts "Amazon VPC vended logs" into the fields of a flow log
//! record, typed — counts, ports, protocol number and window instants as JSON numbers, the
//! identifiers, addresses and status words as JSON strings — and leaves out a field the
//! record could not compute; the structural processors each apply the one transformation
//! their model members state.
//!
//! The recorded flow this was derived from configures `[{"parseVPC": {}}]` with a filter
//! matching everything and reads one full and one unavailable-field record off a Kinesis
//! stream; those are two rows of the table below. The rows differ along what the rule
//! generalises over: the processor chain (a vended parser, a parse-and-mutate chain, a
//! delimited parser), which fields the record supplies, whether the filter applies on
//! transformed logs at all, whether the group carries a transformer, and whether the
//! filter's pattern selects the transformed event.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::base_ms;
use crate::subscription_fan_out::{past_activation, role_arn, stream_arn, Emulator};

/// The log stream every case writes to, and the filter every case attaches.
const LOG_STREAM: &str = "flow-logs";
const FILTER: &str = "to-the-stream";

/// A VPC flow log record in the DEFAULT format — "version account-id interface-id srcaddr
/// dstaddr srcport dstport protocol packets bytes start end action log-status" — with every
/// field supplied.
pub(crate) const FULL_RECORD: &str = "2 123456789012 eni-0a1b2c3d4e5f60718 10.0.1.5 10.0.2.9 443 49152 \
                           6 10 840 1700000000 1700000060 ACCEPT OK";

/// The same format for an interval the interface captured no traffic in: every field the
/// record could not compute carries the `-` placeholder.
const NODATA_RECORD: &str =
    "2 123456789012 eni-0a1b2c3d4e5f60718 - - - - - - - 1700000000 1700000060 - NODATA";

/// A record of the same format whose `action` is the other value a flow log reports, used to
/// show the filter pattern running against the TRANSFORMED event.
pub(crate) const REJECTED_RECORD: &str = "2 123456789012 eni-0a1b2c3d4e5f60718 10.0.3.7 10.0.4.1 53 \
                               33000 17 2 160 1700000100 1700000160 REJECT OK";

/// What a row expects at the destination for the one event it writes.
enum Expected {
    /// The delivered message parses as JSON and IS this document: the field set, the names
    /// and the JSON types all pinned.
    Document(Value),
    /// The delivered message is this text, byte for byte — the event as ingested.
    Text(&'static str),
    /// Nothing is delivered: the filter's pattern selected no event.
    Nothing,
}

/// One row of the table.
struct Row {
    /// What the row varies, named in every assertion it makes.
    case: &'static str,
    /// The log group the row writes to, one per row so the destination's records can be
    /// attributed back to it.
    group: &'static str,
    /// The `transformerConfig` the row puts on the group, where it puts one.
    transformer: Option<Value>,
    /// The `applyOnTransformedLogs` the filter is configured with.
    apply_on_transformed_logs: bool,
    /// The filter's `filterPattern`.
    pattern: &'static str,
    /// The one message the row ingests.
    message: &'static str,
    /// What the destination must hold for it.
    expected: Expected,
}

/// The whole flow log record as `parseVPC` converts it: the camelCase field names, with the
/// counters, ports, protocol number and window instants as JSON NUMBERS.
pub(crate) fn full_record_document() -> Value {
    json!({
        "version": 2,
        "accountId": "123456789012",
        "interfaceId": "eni-0a1b2c3d4e5f60718",
        "srcAddr": "10.0.1.5",
        "dstAddr": "10.0.2.9",
        "srcPort": 443,
        "dstPort": 49152,
        "protocol": 6,
        "packets": 10,
        "bytes": 840,
        "start": 1_700_000_000,
        "end": 1_700_000_060,
        "action": "ACCEPT",
        "logStatus": "OK"
    })
}

/// The same for the record that reports no data: every placeholder field is ABSENT rather
/// than carrying `-` or null.
fn nodata_record_document() -> Value {
    json!({
        "version": 2,
        "accountId": "123456789012",
        "interfaceId": "eni-0a1b2c3d4e5f60718",
        "start": 1_700_000_000,
        "end": 1_700_000_060,
        "logStatus": "NODATA"
    })
}

/// The same for the rejected record.
pub(crate) fn rejected_record_document() -> Value {
    json!({
        "version": 2,
        "accountId": "123456789012",
        "interfaceId": "eni-0a1b2c3d4e5f60718",
        "srcAddr": "10.0.3.7",
        "dstAddr": "10.0.4.1",
        "srcPort": 53,
        "dstPort": 33_000,
        "protocol": 17,
        "packets": 2,
        "bytes": 160,
        "start": 1_700_000_100,
        "end": 1_700_000_160,
        "action": "REJECT",
        "logStatus": "OK"
    })
}

/// A one-processor chain naming `parseVPC`.
pub(crate) fn parse_vpc() -> Value {
    json!([{"parseVPC": {}}])
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case.
            case: "parseVPC on a full flow log record",
            group: "/aws/emulator/vpc-full",
            transformer: Some(parse_vpc()),
            apply_on_transformed_logs: true,
            pattern: "",
            message: FULL_RECORD,
            expected: Expected::Document(full_record_document()),
        },
        Row {
            case: "parseVPC on a record that reports no data",
            group: "/aws/emulator/vpc-nodata",
            transformer: Some(parse_vpc()),
            apply_on_transformed_logs: true,
            pattern: "",
            message: NODATA_RECORD,
            expected: Expected::Document(nodata_record_document()),
        },
        Row {
            case: "a transformed group whose filter does not apply on transformed logs",
            group: "/aws/emulator/vpc-untransformed",
            transformer: Some(parse_vpc()),
            apply_on_transformed_logs: false,
            pattern: "",
            message: FULL_RECORD,
            expected: Expected::Text(FULL_RECORD),
        },
        Row {
            case: "a filter applying on transformed logs on a group with no transformer",
            group: "/aws/emulator/no-transformer",
            transformer: None,
            apply_on_transformed_logs: true,
            pattern: "",
            message: FULL_RECORD,
            expected: Expected::Text(FULL_RECORD),
        },
        Row {
            case: "a pattern that selects the transformed event",
            group: "/aws/emulator/vpc-selected",
            transformer: Some(parse_vpc()),
            apply_on_transformed_logs: true,
            // A JSON condition can only select a JSON event, so a pattern that matches here
            // matched the TRANSFORMED reading: the ingested record is not JSON at all.
            pattern: "{$.action = \"REJECT\"}",
            message: REJECTED_RECORD,
            expected: Expected::Document(rejected_record_document()),
        },
        Row {
            case: "a pattern that selects no transformed event",
            group: "/aws/emulator/vpc-unselected",
            transformer: Some(parse_vpc()),
            apply_on_transformed_logs: true,
            pattern: "{$.action = \"REJECT\"}",
            message: FULL_RECORD,
            expected: Expected::Nothing,
        },
        Row {
            case: "a parse-and-mutate chain",
            group: "/aws/emulator/json-chain",
            transformer: Some(json!([
                {"parseJSON": {}},
                {"renameKeys": {"entries": [{"key": "lvl", "renameTo": "level"}]}},
                {"addKeys": {"entries": [{"key": "env", "value": "prod"}]}},
                {"upperCaseString": {"withKeys": ["level"]}},
                {"copyValue": {"entries": [{"source": "msg", "target": "summary"}]}},
                {"deleteKeys": {"withKeys": ["noise"]}},
            ])),
            apply_on_transformed_logs: true,
            pattern: "",
            message: r#"{"lvl":"warn","noise":"drop me","msg":"slow"}"#,
            expected: Expected::Document(json!({
                "level": "WARN",
                "msg": "slow",
                "summary": "slow",
                "env": "prod",
            })),
        },
        Row {
            case: "a delimited parser naming its columns",
            group: "/aws/emulator/csv-columns",
            transformer: Some(json!([
                {"csv": {"columns": ["service", "status", "latency"]}},
            ])),
            apply_on_transformed_logs: true,
            pattern: "",
            message: "checkout,fail,\"1,200\"",
            expected: Expected::Document(json!({
                "service": "checkout",
                "status": "fail",
                // The quoted column keeps its embedded delimiter.
                "latency": "1,200",
            })),
        },
        Row {
            case: "a key-value parser with its own delimiters",
            group: "/aws/emulator/key-value",
            transformer: Some(json!([
                {"parseKeyValue": {"fieldDelimiter": ";", "keyValueDelimiter": ":",
                                   "keyPrefix": "kv_"}},
            ])),
            apply_on_transformed_logs: true,
            pattern: "",
            message: "svc:search;status:ok",
            expected: Expected::Document(json!({
                "kv_svc": "search",
                "kv_status": "ok",
            })),
        },
    ]
}

/// The messages each log group's delivered envelopes carry, in the order they arrived.
fn delivered_by_group(case: &str, records: &[(String, Value)]) -> BTreeMap<String, Vec<String>> {
    let mut by_group: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_key, envelope) in records {
        let group = envelope["logGroup"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: an envelope names its log group, got {envelope}"))
            .to_string();
        let messages = by_group.entry(group).or_default();
        for event in envelope["logEvents"].as_array().cloned().unwrap_or_default() {
            messages.push(
                event["message"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a delivered event carries a message, got {event}")
                    })
                    .to_string(),
            );
        }
    }
    by_group
}

/// Every row of the table: what the destination is handed, and that the stored event is the
/// ingested one whatever the group transforms.
#[tokio::test]
async fn a_subscription_filter_delivers_the_reading_it_was_applied_to() {
    let emulator = Emulator::new();
    let stream = "transformed-delivery";
    emulator.create_stream("the destination stream", stream).await;
    tokio::time::sleep(past_activation()).await;

    let rows = rows();
    let at = base_ms();
    for row in &rows {
        let case = row.case;
        emulator
            .logs(case, "CreateLogGroup", &json!({"logGroupName": row.group}))
            .await;
        emulator
            .logs(
                case,
                "CreateLogStream",
                &json!({"logGroupName": row.group, "logStreamName": LOG_STREAM}),
            )
            .await;
        if let Some(transformer) = &row.transformer {
            emulator
                .logs(
                    case,
                    "PutTransformer",
                    &json!({"logGroupIdentifier": row.group, "transformerConfig": transformer}),
                )
                .await;
        }
        emulator
            .logs(
                case,
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": row.group,
                    "filterName": FILTER,
                    "filterPattern": row.pattern,
                    "destinationArn": stream_arn(stream),
                    "roleArn": role_arn(),
                    "applyOnTransformedLogs": row.apply_on_transformed_logs,
                }),
            )
            .await;
        emulator
            .logs(
                case,
                "PutLogEvents",
                &json!({
                    "logGroupName": row.group,
                    "logStreamName": LOG_STREAM,
                    "logEvents": [{"timestamp": at, "message": row.message}],
                }),
            )
            .await;

        // A transformer is a second READING of an ingested event: what the group stored is
        // the message the batch carried, byte for byte.
        let read = emulator
            .logs(
                case,
                "GetLogEvents",
                &json!({"logGroupName": row.group, "logStreamName": LOG_STREAM}),
            )
            .await;
        let stored: Vec<String> = read["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| event["message"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            stored,
            vec![row.message.to_string()],
            "case {case}: the stored event is the ingested one, got {stored:?}"
        );
    }

    let records = emulator.delivered("the delivered records", stream).await;
    let delivered = delivered_by_group("the delivered records", &records);
    for row in &rows {
        let case = row.case;
        let carried = delivered.get(row.group).cloned().unwrap_or_default();
        match &row.expected {
            Expected::Nothing => assert!(
                carried.is_empty(),
                "case {case}: a pattern selecting no transformed event delivers nothing, got \
                 {carried:?}"
            ),
            Expected::Text(text) => assert_eq!(
                carried,
                vec![(*text).to_string()],
                "case {case}: expected the event as ingested, got {carried:?}"
            ),
            Expected::Document(document) => {
                assert_eq!(
                    carried.len(),
                    1,
                    "case {case}: one event owes its filter one delivered message, got \
                     {carried:?}"
                );
                let message = &carried[0];
                let parsed: Value = serde_json::from_str(message).unwrap_or_else(|error| {
                    panic!(
                        "case {case}: a filter applying on transformed logs must deliver the \
                         transformed JSON event, got {error} for {message}"
                    )
                });
                assert_eq!(
                    &parsed, document,
                    "case {case}: expected the transformed event {document}, got {message}"
                );
            }
        }
    }
}
