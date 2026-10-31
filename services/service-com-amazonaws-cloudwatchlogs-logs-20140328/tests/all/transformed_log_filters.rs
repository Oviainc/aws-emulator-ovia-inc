// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH VERSION of an ingested log event a log group's filters are
//! applied to once that group carries a TRANSFORMER.
//!
//! RULE: a log group's transformer transforms every log event at ingestion
//! (`PutTransformer`: "CloudWatch Logs performs the transformations at the time of log
//! ingestion"), and a filter the account put with `applyOnTransformedLogs` is applied to
//! that TRANSFORMED version — "if the log group uses either a log-group level or
//! account-level transformer, and you specify `true`, the metric filter will be applied on
//! the transformed version of the log events instead of the original ingested log events" —
//! while every other filter is applied to the message exactly as it was ingested. The member
//! "is valid only for log groups that have an active log transformer", so a group with none
//! leaves every filter reading the ingested message. The rule is the FILTER's, not one
//! operation's: `PutMetricFilter` and `PutSubscriptionFilter` both carry the member, and both
//! are driven below.
//!
//! The metric-filter table spans what the rule generalises over: the processor kind that
//! structures the event (`parseJSON` at the root and into a destination, `parseKeyValue`,
//! `csv`), the mutate processors that follow it (`renameKeys`, `addKeys`, `typeConverter`,
//! `deleteKeys`), the FORM of the filter pattern (a JSON condition on a produced field, a
//! nested one, an unstructured term), the form of the metric value (a literal and a selector
//! resolved on the event), and both directions of the difference — a field only the
//! transformed event carries, and a field only the ingested one does. Every row is run twice
//! over the same ingested event, through a filter that applies on transformed logs and a
//! filter that does not, so each row states both halves of the rule.
//!
//! Every write goes over the wire — `POST /` with `X-Amz-Target: Logs_20140328.<Operation>`
//! through the router that mounts this service — and the datapoints are read from the
//! CloudWatch emulator's own metric store on the shared engine, which is where they have to
//! land for `GetMetricStatistics` to answer with them.

use serde_json::{json, Value};

use crate::harness::{base_ms, group_with_stream, Logs};
use crate::metric_filter_extraction::published;
use crate::subscription_fan_out::{
    past_activation, role_arn, stream_arn, Emulator, BY_LOG_STREAM, DATA_MESSAGE,
};

/// One row of the metric-filter table.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The `transformerConfig` the log group carries.
    transformer: Value,
    /// The `filterPattern` both filters are configured with.
    pattern: &'static str,
    /// The transformation's `metricValue`, the same for both filters.
    metric_value: &'static str,
    /// The message ingested, as one log event.
    message: &'static str,
    /// The values the filter that applies on TRANSFORMED logs must publish.
    on_transformed: &'static [f64],
    /// The values the filter that applies on the INGESTED message must publish.
    as_ingested: &'static [f64],
}

/// The table: one row per form of the rule.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "parseJSON at the root, then a renamed key the pattern selects on",
            transformer: json!([
                {"parseJSON": {}},
                {"renameKeys": {"entries": [{"key": "lvl", "renameTo": "level"}]}},
            ]),
            pattern: r#"{ $.level = "ERROR" }"#,
            metric_value: "$.latency",
            message: r#"{"lvl":"ERROR","latency":42}"#,
            on_transformed: &[42.0],
            as_ingested: &[],
        },
        Row {
            case: "parseJSON into a destination, selected through the nested path",
            transformer: json!([{"parseJSON": {"source": "@message", "destination": "parsed"}}]),
            pattern: r#"{ $.parsed.status = "fail" }"#,
            metric_value: "$.parsed.bytes",
            message: r#"{"status":"fail","bytes":7}"#,
            on_transformed: &[7.0],
            as_ingested: &[],
        },
        Row {
            case: "parseKeyValue structures a message that is no JSON document at all",
            transformer: json!([{"parseKeyValue": {
                "fieldDelimiter": ";",
                "keyValueDelimiter": "=",
                "keyPrefix": "kv_",
            }}]),
            pattern: r#"{ $.kv_status = "fail" }"#,
            metric_value: "$.kv_bytes",
            message: "status=fail;bytes=12",
            on_transformed: &[12.0],
            as_ingested: &[],
        },
        Row {
            case: "csv names the columns of a comma-separated message",
            transformer: json!([{"csv": {"columns": ["ip", "code", "size"]}}]),
            pattern: r#"{ $.code = "500" }"#,
            metric_value: "$.size",
            message: "10.0.0.1,500,256",
            on_transformed: &[256.0],
            as_ingested: &[],
        },
        Row {
            case: "a key addKeys added, with the value typeConverter retyped",
            transformer: json!([
                {"parseJSON": {}},
                {"addKeys": {"entries": [{"key": "env", "value": "prod"}]}},
                {"typeConverter": {"entries": [{"key": "count", "type": "integer"}]}},
            ]),
            pattern: r#"{ $.env = "prod" }"#,
            metric_value: "$.count",
            message: r#"{"count":"5"}"#,
            on_transformed: &[5.0],
            as_ingested: &[],
        },
        Row {
            // The other direction: the ingested event carries a field the transformed one
            // does not, so it is the INGESTED filter that publishes.
            case: "a key deleteKeys removed is matched only on the ingested message",
            transformer: json!([
                {"parseJSON": {}},
                {"deleteKeys": {"withKeys": ["secret"]}},
            ]),
            pattern: r#"{ $.secret = "x" }"#,
            metric_value: "1",
            message: r#"{"secret":"x","n":3}"#,
            on_transformed: &[],
            as_ingested: &[1.0],
        },
        Row {
            // A message the pipeline structures nothing out of has no transformed version,
            // so both filters read the message as it was ingested.
            case: "a message the pipeline extracts nothing from leaves both filters alike",
            transformer: json!([{"parseJSON": {}}]),
            pattern: "ERROR",
            metric_value: "1",
            message: "plain text ERROR",
            on_transformed: &[1.0],
            as_ingested: &[1.0],
        },
    ]
}

/// The metric filter to put: the same pattern and transformation either way, differing only
/// in `applyOnTransformedLogs` and in the metric it publishes to.
fn filter_body(group: &str, name: &str, row: &Row, namespace: &str, on_transformed: bool) -> Value {
    json!({
        "logGroupName": group,
        "filterName": name,
        "filterPattern": row.pattern,
        "applyOnTransformedLogs": on_transformed,
        "metricTransformations": [{
            "metricName": METRIC,
            "metricNamespace": namespace,
            "metricValue": row.metric_value,
        }],
    })
}

/// The metric name every row publishes under; the namespace is what tells the two filters
/// of one row apart.
const METRIC: &str = "Extracted";

/// The values published under one metric, in ascending order — what a row compares against.
fn values(logs: &Logs, namespace: &str) -> Vec<f64> {
    let mut found: Vec<f64> = published(logs, namespace, METRIC)
        .iter()
        .map(|datapoint| datapoint.value)
        .collect();
    found.sort_by(f64::total_cmp);
    found
}

/// A filter put with `applyOnTransformedLogs` extracts from the version the group's
/// transformer produced, and a filter put without it from the message as ingested — over
/// every form of pipeline, pattern and metric value in the table.
#[tokio::test]
async fn a_filter_reads_the_version_of_the_event_its_configuration_names() {
    let logs = Logs::new();
    let at = base_ms();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let group = format!("/transformed-filters/row-{index}");
        let on_transformed_namespace = format!("TransformedFilters{index}OnTransformed");
        let as_ingested_namespace = format!("TransformedFilters{index}AsIngested");
        group_with_stream(&logs, &group, "events", None).await;
        logs.ok(
            "PutTransformer",
            &json!({"logGroupIdentifier": &group, "transformerConfig": row.transformer}),
        )
        .await;
        logs.ok(
            "PutMetricFilter",
            &filter_body(
                &group,
                "on-transformed",
                row,
                &on_transformed_namespace,
                true,
            ),
        )
        .await;
        logs.ok(
            "PutMetricFilter",
            &filter_body(&group, "as-ingested", row, &as_ingested_namespace, false),
        )
        .await;

        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": &group,
                "logStreamName": "events",
                "logEvents": [{"timestamp": at, "message": row.message}],
            }),
        )
        .await;

        let found = values(&logs, &on_transformed_namespace);
        assert_eq!(
            found,
            row.on_transformed.to_vec(),
            "case {case:?}: the filter applying on transformed logs must publish {:?}, got {found:?}",
            row.on_transformed
        );
        let found = values(&logs, &as_ingested_namespace);
        assert_eq!(
            found,
            row.as_ingested.to_vec(),
            "case {case:?}: the filter applying on the ingested message must publish {:?}, got {found:?}",
            row.as_ingested
        );

        // The transformation is an ingest-time reading of the event, not a rewrite of what
        // the group holds: the stream still serves the message as it was ingested.
        let read = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupName": &group, "logStreamName": "events"}),
            )
            .await;
        let stored: Vec<&str> = read["events"]
            .as_array()
            .map(|events| {
                events
                    .iter()
                    .filter_map(|event| event["message"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            stored,
            vec![row.message],
            "case {case:?}: the ingested message is what the stream serves, got {stored:?}"
        );
    }
}

/// Ingest one `{"lvl":"ERROR"}` event, stamped `offset` milliseconds after `at`.
async fn ingest_lvl_error(logs: &Logs, group: &str, at: i64, offset: i64) {
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": "events",
            "logEvents": [{"timestamp": at + offset, "message": r#"{"lvl":"ERROR"}"#}],
        }),
    )
    .await;
}

/// `applyOnTransformedLogs` "is valid only for log groups that have an active log
/// transformer": the same filter on the same group reads the ingested message before a
/// transformer is put and after the transformer is deleted, and the transformed version only
/// in between.
#[tokio::test]
async fn the_transformed_version_exists_only_while_the_group_has_a_transformer() {
    let logs = Logs::new();
    const GROUP: &str = "/transformed-filters/lifecycle";
    const NAMESPACE: &str = "TransformedFilterLifecycle";
    let at = base_ms();
    group_with_stream(&logs, GROUP, "events", None).await;

    // The pattern selects a field only the TRANSFORMER produces, so a datapoint appears
    // exactly when the transformation is in force.
    logs.ok(
        "PutMetricFilter",
        &json!({
            "logGroupName": GROUP,
            "filterName": "on-transformed",
            "filterPattern": r#"{ $.level = "ERROR" }"#,
            "applyOnTransformedLogs": true,
            "metricTransformations": [{
                "metricName": METRIC,
                "metricNamespace": NAMESPACE,
                "metricValue": "1",
            }],
        }),
    )
    .await;

    ingest_lvl_error(&logs, GROUP, at, 0).await;
    let found = values(&logs, NAMESPACE);
    assert!(
        found.is_empty(),
        "case \"before the transformer\": a group with no active transformer leaves the filter \
         on the ingested message, got {found:?}"
    );

    logs.ok(
        "PutTransformer",
        &json!({
            "logGroupIdentifier": GROUP,
            "transformerConfig": [
                {"parseJSON": {}},
                {"renameKeys": {"entries": [{"key": "lvl", "renameTo": "level"}]}},
            ],
        }),
    )
    .await;
    ingest_lvl_error(&logs, GROUP, at, 1).await;
    let found = values(&logs, NAMESPACE);
    assert_eq!(
        found,
        vec![1.0],
        "case \"with the transformer\": the filter reads the transformed event, got {found:?}"
    );

    // "As soon as you do this, the transformation of incoming log events according to that
    // transformer stops" (`DeleteTransformer`).
    logs.ok("DeleteTransformer", &json!({"logGroupIdentifier": GROUP}))
        .await;
    ingest_lvl_error(&logs, GROUP, at, 2).await;
    let found = values(&logs, NAMESPACE);
    assert_eq!(
        found,
        vec![1.0],
        "case \"after the transformer is deleted\": the transformed version is gone, so the \
         event publishes nothing more, got {found:?}"
    );
}

/// The transformation is BOUNDED: an expanding `substituteString` chain is applied only as
/// far as a log event may grow, and is abandoned past it rather than inflating one ingested
/// event without limit. "Each log event can be no larger than 1 MB" (`PutLogEvents`) is the
/// size a transformed event is held to, so a stage whose result fits is applied and a stage
/// whose result would not is dropped — leaving the field as the previous stage left it — and
/// the rest of the pipeline still runs.
#[tokio::test]
async fn an_expanding_substitution_chain_is_bounded_by_what_an_event_may_hold() {
    let logs = Logs::new();
    const GROUP: &str = "/transformed-filters/bounded";
    let at = base_ms();
    group_with_stream(&logs, GROUP, "events", None).await;

    // `SubstituteStringEntry$to` is modelled `@length(min: 1, max: 128)`, so each stage
    // multiplies the field by at most 128: one character becomes 128, then 16,384, and the
    // third stage's 2,097,152 is past the megabyte a log event may hold.
    let replacement = |character: char| character.to_string().repeat(128);
    logs.ok(
        "PutTransformer",
        &json!({
            "logGroupIdentifier": GROUP,
            "transformerConfig": [
                {"parseJSON": {}},
                {"substituteString": {"entries": [
                    {"source": "text", "from": "a", "to": replacement('b')},
                ]}},
                {"substituteString": {"entries": [
                    {"source": "text", "from": "b", "to": replacement('c')},
                ]}},
                {"substituteString": {"entries": [
                    {"source": "text", "from": "c", "to": replacement('d')},
                ]}},
                {"addKeys": {"entries": [{"key": "marker", "value": "bounded"}]}},
            ],
        }),
    )
    .await;

    // (case, pattern, the values the filter must publish)
    let rows: [(&str, &str, &[f64]); 3] = [
        (
            "the stage that fits is applied",
            "cccccccccc",
            &[1.0],
        ),
        (
            "the stage that would not fit is abandoned",
            "dddddddddd",
            &[],
        ),
        (
            "the processors behind it still run",
            r#"{ $.marker = "bounded" }"#,
            &[1.0],
        ),
    ];
    for (index, (_case, pattern, _expected)) in rows.iter().enumerate() {
        logs.ok(
            "PutMetricFilter",
            &json!({
                "logGroupName": GROUP,
                "filterName": format!("bounded-{index}"),
                "filterPattern": pattern,
                "applyOnTransformedLogs": true,
                "metricTransformations": [{
                    "metricName": METRIC,
                    "metricNamespace": format!("TransformedFilterBounded{index}"),
                    "metricValue": "1",
                }],
            }),
        )
        .await;
    }

    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": GROUP,
            "logStreamName": "events",
            "logEvents": [{"timestamp": at, "message": r#"{"text":"a"}"#}],
        }),
    )
    .await;

    for (index, (case, _pattern, expected)) in rows.iter().enumerate() {
        let namespace = format!("TransformedFilterBounded{index}");
        let found = values(&logs, &namespace);
        assert_eq!(
            found,
            expected.to_vec(),
            "case {case:?}: expected {expected:?}, got {found:?}"
        );
    }
}

/// The same rule for the other filter that carries the member: a SUBSCRIPTION filter put
/// with `applyOnTransformedLogs` delivers the transformed version of the event, and one put
/// without it delivers the message as ingested — both from the same batch, to the same
/// destination, which is what tells the two envelopes apart.
#[tokio::test]
async fn a_subscription_filter_delivers_the_version_its_configuration_names() {
    let emulator = Emulator::new();
    let case = "two subscription filters, one on transformed logs";
    let stream = "transformed-fan-out";
    let group = "transformed-subscriptions";
    let log_stream = "events";
    let message = r#"{"lvl":"ERROR"}"#;

    emulator.create_stream(case, stream).await;
    emulator
        .logs(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    emulator
        .logs(
            case,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": log_stream}),
        )
        .await;
    emulator
        .logs(
            case,
            "PutTransformer",
            &json!({
                "logGroupIdentifier": group,
                "transformerConfig": [
                    {"parseJSON": {}},
                    {"addKeys": {"entries": [{"key": "env", "value": "prod"}]}},
                ],
            }),
        )
        .await;
    for (filter, on_transformed) in [("on-transformed", true), ("as-ingested", false)] {
        emulator
            .logs(
                case,
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": filter,
                    "filterPattern": "",
                    "destinationArn": stream_arn(stream),
                    "roleArn": role_arn(),
                    "distribution": BY_LOG_STREAM,
                    "applyOnTransformedLogs": on_transformed,
                }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    emulator
        .logs(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": log_stream,
                "logEvents": [{"timestamp": base_ms(), "message": message}],
            }),
        )
        .await;

    let records = emulator.delivered(case, stream).await;
    assert_eq!(
        records.len(),
        2,
        "case {case}: each of the group's two filters owes one envelope, got {records:?}"
    );
    for (_key, envelope) in &records {
        assert_eq!(
            envelope["messageType"].as_str(),
            Some(DATA_MESSAGE),
            "case {case}: an envelope of ingested events is a {DATA_MESSAGE}, got {envelope}"
        );
        let filter = envelope["subscriptionFilters"][0]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the envelope names its filter, got {envelope}"));
        let delivered = envelope["logEvents"][0]["message"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the envelope carries the event, got {envelope}"));
        let expected = if filter == "on-transformed" {
            json!({"lvl": "ERROR", "env": "prod"})
        } else {
            json!({"lvl": "ERROR"})
        };
        let actual: Value = serde_json::from_str(delivered).unwrap_or_else(|error| {
            panic!("case {case}: {filter} delivered {delivered:?}, which is no JSON ({error})")
        });
        assert_eq!(
            actual, expected,
            "case {case}: {filter} must deliver {expected}, got {actual}"
        );
    }
}
