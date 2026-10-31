// SPDX-License-Identifier: CC0-1.0

//! Each AWS/Events per-rule counter is served under exactly the dimension set(s)
//! EventBridge publishes THAT counter under, end to end over the wire with
//! CloudWatch mounted beside EventBridge on one engine.
//!
//! The RULE asserted here: EventBridge publishes a rule's routing counters
//! (`MatchedEvents`, `TriggeredRules`) under the rule's whole identity —
//! `EventBusName` together with `RuleName` — and its handover counters
//! (`Invocations`, `SuccessfulInvocationAttempts`, `FailedInvocations`) under that
//! identity AND under `RuleName` on its own; every other dimension set a client
//! may name — the bus with no rule, the rule name alone for a routing counter,
//! another rule's name — is a series the counter is not published under, so a
//! read of it answers with no datapoint at all rather than with a zero one
//! ("CloudWatch treats each unique combination of dimensions as a separate
//! metric", `GetMetricStatistics$Dimensions`).
//!
//! The counters are the ones this crate names
//! (`service_com_amazonaws_eventbridge_awsevents::state`'s `*_METRIC` /
//! `*_ATTEMPTS_METRIC` constants); the dimension names are `EventBusName` and
//! `RuleName` from the same module.
//!
//! The table spans the class rather than one case: its rows differ along the BUS
//! the rules live on (two custom buses, one of them carrying a dot, and the
//! default one), along HOW MANY matching events the round publishes (so a count
//! is a count rather than a flag), and the dimension-set table is asserted for
//! every counter on the rule that matched AND on the sibling that did not.
//!
//! The harness (the router with EventBridge, SQS and CloudWatch on one engine) is
//! the one `target_dispatch` owns.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    DEFAULT_EVENT_BUS_NAME, EVENT_BUS_NAME_DIMENSION, FAILED_INVOCATIONS_METRIC,
    INVOCATIONS_METRIC, MATCHED_EVENTS_METRIC, METRIC_NAMESPACE, RULE_NAME_DIMENSION,
    SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC, TRIGGERED_RULES_METRIC,
};

use crate::target_dispatch::{
    bus_dimensions, cloudwatch, drain, emulator, events, given_queue, metric_sum_over, ordered,
    published_series, rule_dimensions, rule_name_dimensions, rule_series, series_filtered_by,
    Emulator, DETAIL_TYPE, SOURCE,
};

/// The rule the published events match, and the one on the same bus they do not.
const MATCHING_RULE: &str = "matching";
const SIBLING_RULE: &str = "sibling";

/// The source the sibling rule pins, which nothing published below carries.
const OTHER_SOURCE: &str = "com.demo.payments";

/// Create `bus` unless it is the one every account already holds.
async fn given_bus(emulator: &Emulator, case: &str, bus: &str) {
    if bus == DEFAULT_EVENT_BUS_NAME {
        return;
    }
    let (status, created) = events(emulator, "CreateEventBus", &json!({ "Name": bus })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateEventBus answered {status} {created}"
    );
}

/// A rule called `name` on `bus` pinning `source`, carrying `target_arn` as its
/// one target.
async fn given_rule(
    emulator: &Emulator,
    case: &str,
    bus: &str,
    name: &str,
    source: &str,
    target_arn: &str,
) {
    let pattern = json!({ "source": [source], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": pattern,
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: PutRule answered {status} {body}");
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": name,
            "EventBusName": bus,
            "Targets": [{ "Id": "t1", "Arn": target_arn }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets reported a failed entry: {body}"
    );
}

/// Publish one matching event onto `bus`, carrying `nonce` in its detail.
async fn publish_onto(emulator: &Emulator, case: &str, bus: &str, nonce: &str) {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": bus,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "nonce": nonce }).to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
}

/// One row: the bus the two rules live on, and how many matching events the
/// round publishes.
struct Row {
    case: &'static str,
    bus: &'static str,
    events: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a custom bus, one event",
            bus: "orders-bus",
            events: 1,
        },
        Row {
            case: "the default bus, two events",
            bus: DEFAULT_EVENT_BUS_NAME,
            events: 2,
        },
        Row {
            case: "a custom bus whose name carries a dot, three events",
            bus: "payments.bus-2",
            events: 3,
        },
    ]
}

/// The counters a rule on a bus reports, each with what it counts for a rule
/// whose queue target accepted every matched event, and whether EventBridge
/// publishes it under the rule name ALONE beside the rule's whole identity.
///
/// A LITERAL table, stated here rather than asked of the service: an expectation
/// computed from the code under test would move with a regression instead of
/// catching it. The counters are the five this crate names; the two routing ones
/// come first.
fn counters(matched: f64) -> [(&'static str, f64, bool); 5] {
    [
        (MATCHED_EVENTS_METRIC, matched, false),
        (TRIGGERED_RULES_METRIC, matched, false),
        (INVOCATIONS_METRIC, matched, true),
        (SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC, matched, true),
        // The queue accepted every handover, so nothing failed permanently and
        // this counter has no series for this rule at all.
        (FAILED_INVOCATIONS_METRIC, 0.0, true),
    ]
}

/// The `Sum` of each of `metrics` for the series `dimensions` names, read the
/// OTHER way a client reads a series — one `GetMetricData` call carrying one
/// `MetricStat` query per metric — beside the `StatusCode` each result carries.
async fn metric_data_sums(
    emulator: &Emulator,
    case: &str,
    metrics: &[&str],
    dimensions: &Value,
) -> Vec<(f64, String)> {
    let now = epoch_seconds();
    let queries: Vec<Value> = metrics
        .iter()
        .enumerate()
        .map(|(index, metric)| {
            json!({
                "Id": format!("m{index}"),
                "MetricStat": {
                    "Metric": {
                        "Namespace": METRIC_NAMESPACE,
                        "MetricName": metric,
                        "Dimensions": dimensions,
                    },
                    "Period": 60,
                    "Stat": "Sum",
                },
                "ReturnData": true,
            })
        })
        .collect();
    let (status, answer) = cloudwatch(
        emulator,
        "GetMetricData",
        &json!({
            "MetricDataQueries": queries,
            "StartTime": now - 300.0,
            "EndTime": now + 300.0,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricData answered {status} {answer}"
    );
    let results = answer["MetricDataResults"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        results.len(),
        metrics.len(),
        "case {case:?}: expected one result per query, got {answer}"
    );
    results
        .iter()
        .map(|result| {
            let sum = result["Values"]
                .as_array()
                .map(|values| values.iter().filter_map(Value::as_f64).sum::<f64>())
                .unwrap_or_default();
            let code = result["StatusCode"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (sum, code)
        })
        .collect()
}

/// Seconds since the epoch, as a read's `StartTime` / `EndTime` carry it.
fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The datapoints of `metric` for the series `dimensions` names, read the way a
/// client watching a live counter reads it: a wide window whose end is ahead of
/// now, folded into the 60-second periods the counter is published at, asking for
/// the two basic statistics a count is read by.
async fn minute_datapoints(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    dimensions: &Value,
) -> Vec<Value> {
    let now = epoch_seconds();
    let (status, stats) = cloudwatch(
        emulator,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": now - 1_200.0,
            "EndTime": now + 300.0,
            "Period": 60,
            "Statistics": ["Sum", "SampleCount"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics answered {status} {stats}"
    );
    assert_eq!(
        stats["Label"],
        json!(metric),
        "case {case:?}: expected the read to be labelled with the metric, got {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn each_counter_is_served_under_its_own_dimension_sets() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, row.bus).await;
        let (queue_url, queue_arn) = given_queue(&emulator, "deliveries").await;
        given_rule(&emulator, case, row.bus, MATCHING_RULE, SOURCE, &queue_arn).await;
        given_rule(
            &emulator,
            case,
            row.bus,
            SIBLING_RULE,
            OTHER_SOURCE,
            &queue_arn,
        )
        .await;

        for round in 0..row.events {
            publish_onto(&emulator, case, row.bus, &format!("n-{round}")).await;
        }
        // The deliveries themselves, so what the counters count is observable
        // beside them: one message per published event, from the matching rule's
        // target only.
        let delivered = drain(&emulator, &queue_url).await;
        assert_eq!(
            delivered.len(),
            row.events,
            "case {case:?}: expected the matching rule's target to hold every event, \
             got {delivered:?}"
        );

        let matched = f64::from(u32::try_from(row.events).unwrap_or(u32::MAX));
        for (metric, counted, by_name) in counters(matched) {
            // The dimension sets a client may name, and what each answers for
            // the rule that matched: its whole identity always carries the
            // count, the rule name alone carries it only for a counter
            // published under it, and a set no series was published under
            // answers with NO DATAPOINT at all rather than with a zero one.
            let probes: [(&str, Value, f64); 5] = [
                (
                    "the bus and the rule",
                    rule_dimensions(row.bus, MATCHING_RULE),
                    counted,
                ),
                (
                    "the rule name alone",
                    rule_name_dimensions(MATCHING_RULE),
                    if by_name { counted } else { 0.0 },
                ),
                ("the bus alone", bus_dimensions(row.bus), 0.0),
                (
                    "the sibling rule's whole identity",
                    rule_dimensions(row.bus, SIBLING_RULE),
                    0.0,
                ),
                (
                    "the sibling rule's name alone",
                    rule_name_dimensions(SIBLING_RULE),
                    0.0,
                ),
            ];
            for (dimensioned, dimensions, want) in probes {
                let points = minute_datapoints(&emulator, case, metric, &dimensions).await;
                if want == 0.0 {
                    assert!(
                        points.is_empty(),
                        "case {case:?}: expected {metric} read under {dimensioned} to answer \
                         with no datapoint at all, got {points:?}"
                    );
                    continue;
                }
                // Read at the granularity the counter is published at: every
                // occurrence counted once in its own minute, in `Count`.
                let summed: f64 = points
                    .iter()
                    .filter_map(|point| point["Sum"].as_f64())
                    .sum();
                let occurrences: f64 = points
                    .iter()
                    .filter_map(|point| point["SampleCount"].as_f64())
                    .sum();
                assert_eq!(
                    (summed, occurrences),
                    (want, want),
                    "case {case:?}: expected {metric} read under {dimensioned} to hold {want} \
                     occurrence(s) summing to {want}, got {points:?}"
                );
                for point in &points {
                    assert_eq!(
                        point["Unit"],
                        json!("Count"),
                        "case {case:?}: expected every {metric} datapoint under {dimensioned} \
                         in Count, got {point}"
                    );
                }
                // The same window read over a wider period answers the same
                // total, so the count is the series' and not one window's.
                let got = metric_sum_over(&emulator, metric, &dimensions).await;
                assert_eq!(
                    got, want,
                    "case {case:?}: expected {metric} read under {dimensioned} to sum to \
                     {want}, got {got}"
                );
            }
        }

        // The same counts read the OTHER way a client reads a series: one
        // `GetMetricData` call over the rule's whole identity, carrying one
        // query per counter, answers the same sums and reports each result
        // complete.
        let counters = counters(matched);
        let metrics: Vec<&str> = counters.iter().map(|(metric, _, _)| *metric).collect();
        let read = metric_data_sums(
            &emulator,
            case,
            &metrics,
            &rule_dimensions(row.bus, MATCHING_RULE),
        )
        .await;
        for ((metric, counted, _), (got, code)) in counters.iter().zip(read) {
            assert_eq!(
                got, *counted,
                "case {case:?}: expected GetMetricData to answer {counted} for {metric} \
                 under the bus and the rule, got {got}"
            );
            assert_eq!(
                code, "Complete",
                "case {case:?}: expected a complete {metric} result, got {code:?}"
            );
        }

        // And the series CloudWatch holds in the namespace: exactly the matching
        // rule's counters, each under the dimension set(s) that counter is
        // published under, and nothing for the rule that matched nothing.
        let expected_series = rule_series(
            row.bus,
            MATCHING_RULE,
            &[
                MATCHED_EVENTS_METRIC,
                TRIGGERED_RULES_METRIC,
                INVOCATIONS_METRIC,
                SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
            ],
        );
        let series = published_series(&emulator).await;
        assert_eq!(
            ordered(&series),
            ordered(&expected_series),
            "case {case:?}: expected one series per dimension set of each counter the \
             matching rule reports, got {series:?}"
        );
        // The same listing reached through a dimension FILTER on the rule name,
        // which is how a client discovers a rule's series: every one of them
        // carries that dimension, so the filter answers the same set — and the
        // routing counters come back under their two-dimension set, which a
        // filter naming one dimension matches.
        let filtered = series_filtered_by(&emulator, RULE_NAME_DIMENSION, MATCHING_RULE).await;
        assert_eq!(
            ordered(&filtered),
            ordered(&expected_series),
            "case {case:?}: expected the RuleName filter to answer the rule's own series, \
             got {filtered:?}"
        );
        for metric in [MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC] {
            let dimension_names: Vec<Vec<String>> = filtered
                .iter()
                .filter(|found| found["MetricName"] == json!(metric))
                .map(|found| {
                    found["Dimensions"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|dimension| {
                            dimension["Name"].as_str().unwrap_or_default().to_string()
                        })
                        .collect()
                })
                .collect();
            assert_eq!(
                dimension_names,
                vec![vec![
                    EVENT_BUS_NAME_DIMENSION.to_string(),
                    RULE_NAME_DIMENSION.to_string(),
                ]],
                "case {case:?}: expected the one {metric} series the filter answers to carry \
                 exactly the bus and the rule, got {dimension_names:?}"
            );
        }
    }
}
