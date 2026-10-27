// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's ENHANCED SHARD-LEVEL METRICS, end to end over the wire:
//! `CreateStream` -> `EnableEnhancedMonitoring` / `DisableEnhancedMonitoring` ->
//! `DescribeStreamSummary` / `DescribeStream`.
//!
//! RULE: a stream carries a SET of enhanced shard-level metrics, empty until something
//! enhances one. `EnableEnhancedMonitoring` ADDS the metrics its request names to that set
//! and `DisableEnhancedMonitoring` REMOVES them, the catch-all value standing for every
//! metric the model's `MetricsName` enum lists; each call reports the set as it was before
//! the call (`CurrentShardLevelMetrics`) beside the set it leaves behind
//! (`DesiredShardLevelMetrics`) and persists the latter, so whatever a change stored is
//! what every later read of that stream reports. Naming a metric that is already enhanced,
//! or disabling one nothing enhanced, therefore leaves the same set behind. A value the
//! enum does not accept is refused with a 400 and leaves the stored set exactly as it was;
//! either call resolves its stream by `StreamName`, by `StreamARN` or by both inside the
//! region the request was signed for, is `ResourceNotFoundException` for a stream this
//! account and region do not hold, `InvalidArgumentException` for a request naming no
//! stream at all, and `ResourceInUseException` while the stream is not `ACTIVE`.
//!
//! The recorded flows this was derived from enable metrics on ONE stream and ask one
//! invalid metric of the disabling call; those are two rows of the tables below. The rows
//! differ along what the rule generalises over: which operation is called, how many metrics
//! one call names, whether they are already enhanced, whether the catch-all or individual
//! metrics are named, how many calls one stream takes, the identifier the request resolves
//! the stream by, and the status the stream is in when the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    stream_not_found_message, STREAM_CREATING, STREAM_IDENTIFIER_REQUIRED_MESSAGE,
};

use crate::stream_tags::{error_code, error_message, past_activation, stream_arn, Emulator, REGION};

/// Every shard-level metric enhanced monitoring reports, spelled out here rather than read
/// from the service so a catalog that drifts fails this test instead of agreeing with
/// itself: the values of the `MetricsName` enum
/// (`smithy_query(query="shape",shape="MetricsName")`) less the catch-all `ALL`, which
/// "enables every metric" (`EnableEnhancedMonitoringInput$ShardLevelMetrics`) rather than
/// naming one. This is also the order every report of the set is in, so two reads of one
/// stream cannot disagree about it.
const EVERY_METRIC: &[&str] = &[
    "IncomingBytes",
    "IncomingRecords",
    "IteratorAgeMilliseconds",
    "OutgoingBytes",
    "OutgoingRecords",
    "ReadProvisionedThroughputExceeded",
    "WriteProvisionedThroughputExceeded",
];

/// The catch-all value that stands for [`EVERY_METRIC`].
const ALL: &str = "ALL";

/// Which way a step moves the stream's set.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// `EnableEnhancedMonitoring`, which adds to the set.
    Enable,
    /// `DisableEnhancedMonitoring`, which takes away from it.
    Disable,
}

impl Change {
    /// The operation this direction is served by.
    fn operation(self) -> &'static str {
        match self {
            Self::Enable => "EnableEnhancedMonitoring",
            Self::Disable => "DisableEnhancedMonitoring",
        }
    }
}

/// How a step addresses its stream: either identifier resolves it, and so do both together.
#[derive(Debug, Clone, Copy)]
enum By {
    Name,
    Arn,
    Both,
}

impl By {
    /// The request body naming `stream`, carrying `metrics`.
    fn body(self, stream: &str, metrics: &[&str]) -> Value {
        match self {
            Self::Name => json!({ "StreamName": stream, "ShardLevelMetrics": metrics }),
            Self::Arn => json!({
                "StreamARN": stream_arn(REGION, stream),
                "ShardLevelMetrics": metrics,
            }),
            Self::Both => json!({
                "StreamName": stream,
                "StreamARN": stream_arn(REGION, stream),
                "ShardLevelMetrics": metrics,
            }),
        }
    }
}

/// One step of a row: the change it asks for, the metrics it names, how it addresses the
/// stream, and the set the stream must report once it is served.
struct Step {
    change: Change,
    metrics: &'static [&'static str],
    by: By,
    leaves: &'static [&'static str],
}

/// The metrics `expected` names, in the order every report of the set is in.
fn in_catalog_order(expected: &[&str]) -> Vec<String> {
    EVERY_METRIC
        .iter()
        .filter(|metric| expected.contains(metric))
        .map(|metric| (*metric).to_string())
        .collect()
}

/// The `ShardLevelMetrics` list a response member carries, as strings.
fn metric_list(case: &str, response: &Value, member: &str) -> Vec<String> {
    let list = response
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: the response must carry {member}, got {response}"));
    list.iter()
        .map(|metric| {
            metric
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: {member} holds strings, got {metric}"))
                .to_string()
        })
        .collect()
}

/// The enhanced set `DescribeStreamSummary` reports for `stream`.
async fn summarised_metrics(emulator: &Emulator, case: &str, stream: &str) -> Vec<String> {
    let described = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": stream }))
        .await;
    let entries = described["StreamDescriptionSummary"]
        .get("EnhancedMonitoring")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: the summary must carry EnhancedMonitoring, got {described}")
        });
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected one EnhancedMonitoring entry, got {described}"
    );
    metric_list(case, &entries[0], "ShardLevelMetrics")
}

/// The enhanced set `DescribeStream` reports for `stream`.
async fn described_metrics(emulator: &Emulator, case: &str, stream: &str) -> Vec<String> {
    let described = emulator
        .ok(case, "DescribeStream", &json!({ "StreamName": stream }))
        .await;
    let entries = described["StreamDescription"]
        .get("EnhancedMonitoring")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: the description must carry EnhancedMonitoring, got {described}")
        });
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected one EnhancedMonitoring entry, got {described}"
    );
    metric_list(case, &entries[0], "ShardLevelMetrics")
}

/// Every accepted change reports the set it replaced beside the set it stored, and the
/// stored one is what both reads of the stream report afterwards.
#[tokio::test]
async fn a_metric_change_is_what_a_read_reports() {
    // Each row is a sequence of changes applied to ONE stream, each with the set the stream
    // must carry afterwards. A freshly created stream enhances no metric, so every row
    // starts from the empty set.
    let rows: Vec<(&str, Vec<Step>)> = vec![
        (
            "one-metric",
            vec![Step {
                change: Change::Enable,
                metrics: &["IncomingBytes"],
                by: By::Name,
                leaves: &["IncomingBytes"],
            }],
        ),
        (
            "several-metrics-at-once",
            vec![Step {
                change: Change::Enable,
                // Named out of the catalog's order: the request's order is not the set's.
                metrics: &["OutgoingRecords", "IncomingRecords"],
                by: By::Arn,
                leaves: &["IncomingRecords", "OutgoingRecords"],
            }],
        ),
        (
            "the-catch-all",
            vec![Step {
                change: Change::Enable,
                metrics: &[ALL],
                by: By::Both,
                leaves: EVERY_METRIC,
            }],
        ),
        (
            "metrics-accumulate",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["IteratorAgeMilliseconds"],
                    by: By::Name,
                    leaves: &["IteratorAgeMilliseconds"],
                },
                Step {
                    change: Change::Enable,
                    metrics: &["OutgoingBytes", "IncomingBytes"],
                    by: By::Arn,
                    leaves: &["IncomingBytes", "IteratorAgeMilliseconds", "OutgoingBytes"],
                },
            ],
        ),
        (
            "enabling-what-is-already-enhanced-changes-nothing",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["ReadProvisionedThroughputExceeded"],
                    by: By::Name,
                    leaves: &["ReadProvisionedThroughputExceeded"],
                },
                Step {
                    change: Change::Enable,
                    // The same metric twice in one request, and one already enhanced.
                    metrics: &[
                        "ReadProvisionedThroughputExceeded",
                        "ReadProvisionedThroughputExceeded",
                    ],
                    by: By::Name,
                    leaves: &["ReadProvisionedThroughputExceeded"],
                },
            ],
        ),
        (
            "one-metric-back-off-the-catch-all",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &[ALL],
                    by: By::Name,
                    leaves: EVERY_METRIC,
                },
                Step {
                    change: Change::Disable,
                    metrics: &["IncomingBytes"],
                    by: By::Arn,
                    leaves: &[
                        "IncomingRecords",
                        "IteratorAgeMilliseconds",
                        "OutgoingBytes",
                        "OutgoingRecords",
                        "ReadProvisionedThroughputExceeded",
                        "WriteProvisionedThroughputExceeded",
                    ],
                },
            ],
        ),
        (
            "enable-then-disable-the-same-metrics",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["WriteProvisionedThroughputExceeded", "OutgoingBytes"],
                    by: By::Name,
                    leaves: &["OutgoingBytes", "WriteProvisionedThroughputExceeded"],
                },
                Step {
                    change: Change::Disable,
                    metrics: &["OutgoingBytes", "WriteProvisionedThroughputExceeded"],
                    by: By::Name,
                    leaves: &[],
                },
            ],
        ),
        (
            "disabling-what-nothing-enhanced-changes-nothing",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["IncomingRecords"],
                    by: By::Name,
                    leaves: &["IncomingRecords"],
                },
                Step {
                    change: Change::Disable,
                    metrics: &["OutgoingRecords"],
                    by: By::Name,
                    leaves: &["IncomingRecords"],
                },
            ],
        ),
        (
            "the-catch-all-takes-every-metric-off",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["IncomingBytes", "OutgoingBytes", "IteratorAgeMilliseconds"],
                    by: By::Name,
                    leaves: &["IncomingBytes", "IteratorAgeMilliseconds", "OutgoingBytes"],
                },
                Step {
                    change: Change::Disable,
                    metrics: &[ALL],
                    by: By::Both,
                    leaves: &[],
                },
            ],
        ),
        (
            "disabling-nothing-enhanced-at-all",
            vec![Step {
                change: Change::Disable,
                metrics: &["IncomingBytes"],
                by: By::Name,
                leaves: &[],
            }],
        ),
    ];

    let emulator = Emulator::new();
    for (name, _) in &rows {
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for (name, steps) in &rows {
        let mut carried: Vec<String> = Vec::new();
        assert_eq!(
            summarised_metrics(&emulator, name, name).await,
            carried,
            "case {name}: expected a new stream to enhance no metric"
        );

        for (index, step) in steps.iter().enumerate() {
            let case = format!("{name}-{index}-{}", step.change.operation());
            let response = emulator
                .ok(
                    &case,
                    step.change.operation(),
                    &step.by.body(name, step.metrics),
                )
                .await;

            // "The name of the Kinesis data stream" and "the ARN of the stream" are the
            // stream the call resolved, whichever identifier named it.
            assert_eq!(
                response.get("StreamName").and_then(Value::as_str),
                Some(*name),
                "case {case}: expected the response to name the stream, got {response}"
            );
            assert_eq!(
                response.get("StreamARN").and_then(Value::as_str),
                Some(stream_arn(REGION, name).as_str()),
                "case {case}: expected the response to carry the stream's ARN, got {response}"
            );

            let current = metric_list(&case, &response, "CurrentShardLevelMetrics");
            assert_eq!(
                current, carried,
                "case {case}: expected the set before the call, got {current:?}"
            );
            let desired = metric_list(&case, &response, "DesiredShardLevelMetrics");
            let expected = in_catalog_order(step.leaves);
            assert_eq!(
                desired, expected,
                "case {case}: expected the call to leave {expected:?}, got {desired:?}"
            );

            // The stored set is what both reads of the stream report.
            let summarised = summarised_metrics(&emulator, &case, name).await;
            assert_eq!(
                summarised, expected,
                "case {case}: expected the summary to report {expected:?}, got {summarised:?}"
            );
            let described = described_metrics(&emulator, &case, name).await;
            assert_eq!(
                described, expected,
                "case {case}: expected the description to report {expected:?}, got {described:?}"
            );
            carried = expected;
        }
    }
}

/// One refused change: the metrics it names, the stream it addresses and the refusal it
/// must answer.
struct Refusal {
    case: &'static str,
    change: Change,
    metrics: &'static [&'static str],
    code: &'static str,
    /// What the refusal's message must name, built from the row's own stream and metrics.
    says: fn(&str, &[&str]) -> Vec<String>,
    /// Whether the row's stream exists and is `ACTIVE` when the call arrives.
    subject: Subject,
}

/// What the stream a refusal row addresses looks like when the call arrives.
#[derive(Debug, Clone, Copy)]
enum Subject {
    /// An `ACTIVE` stream carrying one enhanced metric, so a refusal that wrote anything
    /// would show.
    Enhanced,
    /// A stream whose create has not fallen due yet.
    Creating,
    /// No stream at all.
    Absent,
}

/// The metrics the `Enhanced` subject carries when the call arrives.
const CARRIED: &[&str] = &["IncomingBytes"];

/// A refused change answers a client error and leaves the stored set exactly as it was.
#[tokio::test]
async fn a_refused_metric_change_leaves_the_set_as_it_was() {
    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "unknown-metric",
            change: Change::Disable,
            metrics: &["BytesIncoming"],
            code: "ValidationException",
            says: |_, metrics| refused_value(metrics),
            subject: Subject::Enhanced,
        },
        Refusal {
            case: "unknown-metric-enabling",
            change: Change::Enable,
            metrics: &["BytesIncoming"],
            code: "ValidationException",
            says: |_, metrics| refused_value(metrics),
            subject: Subject::Enhanced,
        },
        Refusal {
            // The enum's values are case-sensitive, so a metric spelled another way names
            // no metric.
            case: "a-metric-in-another-casing",
            change: Change::Enable,
            metrics: &["incomingbytes"],
            code: "ValidationException",
            says: |_, metrics| refused_value(metrics),
            subject: Subject::Enhanced,
        },
        Refusal {
            case: "the-catch-all-in-another-casing",
            change: Change::Enable,
            metrics: &["all"],
            code: "ValidationException",
            says: |_, metrics| refused_value(metrics),
            subject: Subject::Enhanced,
        },
        Refusal {
            // One good metric does not carry a bad one through: the whole list is refused
            // and nothing is enhanced.
            case: "one-good-metric-beside-a-bad-one",
            change: Change::Enable,
            metrics: &["OutgoingBytes", "SomethingElse"],
            code: "ValidationException",
            says: |_, _| refused_value(&["SomethingElse"]),
            subject: Subject::Enhanced,
        },
        Refusal {
            case: "no-such-stream",
            change: Change::Enable,
            metrics: &["IncomingBytes"],
            code: "ResourceNotFoundException",
            says: |stream, _| vec![stream_not_found_message(stream)],
            subject: Subject::Absent,
        },
        Refusal {
            case: "no-such-stream-disabling",
            change: Change::Disable,
            metrics: &[ALL],
            code: "ResourceNotFoundException",
            says: |stream, _| vec![stream_not_found_message(stream)],
            subject: Subject::Absent,
        },
        Refusal {
            case: "still-creating",
            change: Change::Enable,
            metrics: &["IncomingBytes"],
            code: "ResourceInUseException",
            says: |_, _| vec![STREAM_CREATING.to_string()],
            subject: Subject::Creating,
        },
    ];

    let emulator = Emulator::new();
    for row in &rows {
        if let Subject::Enhanced = row.subject {
            emulator
                .ok(
                    row.case,
                    "CreateStream",
                    &json!({ "StreamName": row.case, "ShardCount": 1 }),
                )
                .await;
        }
    }
    tokio::time::sleep(past_activation()).await;

    for row in &rows {
        let case = row.case;
        match row.subject {
            Subject::Enhanced => {
                emulator
                    .ok(
                        case,
                        "EnableEnhancedMonitoring",
                        &json!({ "StreamName": case, "ShardLevelMetrics": CARRIED }),
                    )
                    .await;
            }
            Subject::Creating => {
                emulator
                    .ok(
                        case,
                        "CreateStream",
                        &json!({ "StreamName": case, "ShardCount": 1 }),
                    )
                    .await;
            }
            Subject::Absent => {}
        }

        let (status, response) = emulator
            .call(
                row.change.operation(),
                &json!({ "StreamName": case, "ShardLevelMetrics": row.metrics }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            row.code,
            "case {case}: expected {}, got {response}",
            row.code
        );
        let message = error_message(&response);
        for says in (row.says)(case, row.metrics) {
            assert!(
                message.contains(&says),
                "case {case}: expected the message to name {says:?}, got {message:?}"
            );
        }

        if let Subject::Enhanced = row.subject {
            let after = summarised_metrics(&emulator, case, case).await;
            assert_eq!(
                after,
                in_catalog_order(CARRIED),
                "case {case}: a refused change must leave the stored set untouched, got \
                 {after:?}"
            );
        }
    }
}

/// What a refusal of a value outside the enum must name: the count of refused values, and
/// each value at fault, taken from the request.
fn refused_value(metrics: &[&str]) -> Vec<String> {
    let mut says = vec!["1 validation error detected".to_string()];
    says.extend(metrics.iter().map(|metric| (*metric).to_string()));
    says.push("shardLevelMetrics".to_string());
    says
}

/// A refusal of a value outside the enum names the value at fault, the member that carried
/// it and every value the enum accepts — the three variable parts of the sentence the
/// service states such a refusal with, taken from the request and from the model.
///
/// The sentence itself is the framework's, stated once before any handler runs for every
/// modelled constraint of every operation; what this pins is that the refusal of this
/// member says which value was refused, where it sat and what it had to be.
#[tokio::test]
async fn an_unsupported_metric_is_refused_with_what_the_enum_accepts() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "wording",
            "CreateStream",
            &json!({ "StreamName": "wording", "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    for operation in ["EnableEnhancedMonitoring", "DisableEnhancedMonitoring"] {
        let (status, response) = emulator
            .call(
                operation,
                &json!({
                    "StreamName": "wording",
                    "ShardLevelMetrics": ["IncomingBytes", "NotAMetric"],
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {operation}: expected the refusal, got {status} with body {response}"
        );
        let message = error_message(&response);
        for says in ["1 validation error detected", "NotAMetric", "shardLevelMetrics"] {
            assert!(
                message.contains(says),
                "case {operation}: expected the message to name {says:?}, got {message:?}"
            );
        }
        for metric in EVERY_METRIC.iter().chain(std::iter::once(&ALL)) {
            assert!(
                message.contains(metric),
                "case {operation}: expected the message to name the accepted value \
                 {metric:?}, got {message:?}"
            );
        }
    }
}

/// A request naming no stream at all is refused by the rule every stream operation applies,
/// before any metric is read or written.
#[tokio::test]
async fn a_metric_change_needs_an_identifier() {
    let emulator = Emulator::new();
    for operation in ["EnableEnhancedMonitoring", "DisableEnhancedMonitoring"] {
        let (status, response) = emulator
            .call(operation, &json!({ "ShardLevelMetrics": ["IncomingBytes"] }))
            .await;
        assert_eq!(
            status, 400,
            "case {operation}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {operation}: expected InvalidArgumentException, got {response}"
        );
        let message = error_message(&response);
        assert!(
            message.contains(STREAM_IDENTIFIER_REQUIRED_MESSAGE),
            "case {operation}: expected the message to name the identifier rule, got \
             {message:?}"
        );
    }
}
