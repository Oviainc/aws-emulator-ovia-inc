// SPDX-License-Identifier: CC0-1.0

//! A pattern MATCH is counted on the matched rule's own AWS/Events metrics, end
//! to end over the wire with CloudWatch mounted beside EventBridge on one engine.
//!
//! The RULE asserted here: for every enabled rule whose `EventPattern` matches a
//! published event, EventBridge publishes one AWS/Events datapoint of value 1 for
//! that event on BOTH `MatchedEvents` and `TriggeredRules`, under each dimension
//! set a rule's counters are read by — the rule's whole identity
//! (`EventBusName` + `RuleName`) and the rule name alone — and attributes it to
//! that one rule, so a sibling rule on the same bus whose pattern did NOT match
//! sums to zero over the same window. The match is counted because the pattern
//! selected the event, not because anything was delivered: a matched rule
//! carrying no target at all counts its matches, and a rule that did deliver
//! counts them beside its `Invocations`.
//!
//! The table spans the class rather than the one recorded case: its rows differ
//! along the BUS the rules live on (two custom buses and the default one), along
//! the ENVELOPE MEMBER whose value tells the matching rule's pattern from its
//! sibling's (`source`, `detail-type`, a field of `detail`), along HOW MANY
//! matching events the round publishes (so a count is a count rather than a
//! flag), and along whether the matching rule carries a TARGET at all.
//!
//! The harness (the router with EventBridge, SQS and CloudWatch on one engine) is
//! the one `target_dispatch` owns.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    DEFAULT_EVENT_BUS_NAME, INVOCATIONS_METRIC, MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC,
};

use crate::target_dispatch::{
    drain, emulator, events, given_queue, metric_sum, metric_sum_over,
    metric_sum_over_last_minute, ordered, published_series, rule_dimensions, rule_name_dimensions,
    rule_series, Emulator,
};

/// The rule whose pattern the published events match, and the one on the same bus
/// whose pattern they do not.
const MATCHING_RULE: &str = "matching";
const SIBLING_RULE: &str = "sibling";

/// The two metrics a match is counted on.
const MATCH_METRICS: [&str; 2] = [MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC];

/// The bus a row's rules live on, created when it is not the default one.
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

/// A rule called `name` on `bus` whose `EventPattern` is `pattern`, carrying
/// `target_arn` as its one target when a row asks for one.
async fn given_rule(
    emulator: &Emulator,
    case: &str,
    bus: &str,
    name: &str,
    pattern: &Value,
    target_arn: Option<&str>,
) {
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({
            "Name": name,
            "EventBusName": bus,
            "EventPattern": pattern.to_string(),
            "State": "ENABLED",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule answered {status} {body}"
    );
    let Some(target_arn) = target_arn else {
        return;
    };
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

/// Publish one event onto `bus` carrying `nonce` beside the row's own detail.
async fn publish_onto(emulator: &Emulator, case: &str, row: &Row, nonce: &str) {
    let mut detail = row.detail.clone();
    if let Some(fields) = detail.as_object_mut() {
        fields.insert("nonce".to_string(), Value::String(nonce.to_string()));
    }
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "EventBusName": row.bus,
                "Source": row.source,
                "DetailType": row.detail_type,
                "Detail": detail.to_string(),
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

/// One row: the bus, the two patterns, the event they are matched against, how
/// many such events the round publishes, and whether the matching rule carries a
/// target.
struct Row {
    case: &'static str,
    bus: &'static str,
    matching: Value,
    sibling: Value,
    source: &'static str,
    detail_type: &'static str,
    detail: Value,
    events: usize,
    targeted: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a custom bus, the patterns differing on source, one event, no target at all",
            bus: "orders-bus",
            matching: json!({ "source": ["com.demo.orders"] }),
            sibling: json!({ "source": ["com.demo.payments"] }),
            source: "com.demo.orders",
            detail_type: "Order Created",
            detail: json!({ "orderId": "A-1" }),
            events: 1,
            targeted: false,
        },
        Row {
            case: "the default bus, the patterns differing on detail-type, three events, \
                   a queue target",
            bus: DEFAULT_EVENT_BUS_NAME,
            matching: json!({ "detail-type": ["Order Created"] }),
            sibling: json!({ "detail-type": ["Order Cancelled"] }),
            source: "com.demo.orders",
            detail_type: "Order Created",
            detail: json!({ "orderId": "B-2" }),
            events: 3,
            targeted: true,
        },
        Row {
            case: "another custom bus, the patterns differing on a field of detail, two events, \
                   no target",
            bus: "payments.bus-2",
            matching: json!({ "detail": { "status": ["shipped"] } }),
            sibling: json!({ "detail": { "status": ["cancelled"] } }),
            source: "com.demo.fulfilment",
            detail_type: "Shipment Updated",
            detail: json!({ "status": "shipped" }),
            events: 2,
            targeted: false,
        },
    ]
}

#[tokio::test]
async fn every_pattern_match_is_counted_on_the_matched_rules_own_metrics() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, row.bus).await;
        let target = if row.targeted {
            Some(given_queue(&emulator, "deliveries").await)
        } else {
            None
        };
        given_rule(
            &emulator,
            case,
            row.bus,
            MATCHING_RULE,
            &row.matching,
            target.as_ref().map(|(_, arn)| arn.as_str()),
        )
        .await;
        given_rule(&emulator, case, row.bus, SIBLING_RULE, &row.sibling, None).await;

        for round in 0..row.events {
            publish_onto(&emulator, case, &row, &format!("n-{round}")).await;
        }

        let expected = f64::from(u32::try_from(row.events).unwrap_or(u32::MAX));
        // The match itself: counted once per matching event on each of the two
        // metrics, for the rule whose pattern selected it and for no other —
        // under the rule's whole identity and under its name alone, which are
        // two series of the same count.
        for metric in MATCH_METRICS {
            for (dimensioned, dimensions) in [
                ("the bus and the rule", rule_dimensions(row.bus, MATCHING_RULE)),
                ("the rule name alone", rule_name_dimensions(MATCHING_RULE)),
            ] {
                let got = metric_sum_over(&emulator, metric, &dimensions).await;
                assert_eq!(
                    got, expected,
                    "case {case:?}: expected {metric} for the matching rule under \
                     {dimensioned} to sum to {expected}, got {got}"
                );
                // The same count read at the granularity the series is published
                // at: the one 60-second period ending now, which is where a
                // datapoint of a just-published event belongs.
                let aligned =
                    metric_sum_over_last_minute(&emulator, metric, &dimensions).await;
                assert_eq!(
                    aligned, expected,
                    "case {case:?}: expected {metric} for the matching rule under \
                     {dimensioned} to sum to {expected} over the minute ending now, \
                     got {aligned}"
                );
            }
            for (dimensioned, dimensions) in [
                ("the bus and the rule", rule_dimensions(row.bus, SIBLING_RULE)),
                ("the rule name alone", rule_name_dimensions(SIBLING_RULE)),
            ] {
                let got = metric_sum_over(&emulator, metric, &dimensions).await;
                assert_eq!(
                    got, 0.0,
                    "case {case:?}: expected {metric} for the rule the events did not match, \
                     under {dimensioned}, to sum to 0, got {got}"
                );
            }
        }

        // What became of the match is a different fact, counted separately: a
        // matched rule with no target invokes nothing and still counts every
        // match above, and one whose queue target accepted the events counts an
        // invocation per delivery beside them.
        let invoked = metric_sum(&emulator, INVOCATIONS_METRIC, row.bus, MATCHING_RULE).await;
        let delivered = if let Some((url, _)) = &target {
            drain(&emulator, url).await.len()
        } else {
            0
        };
        let expected_invocations = if row.targeted { expected } else { 0.0 };
        assert_eq!(
            invoked, expected_invocations,
            "case {case:?}: expected {INVOCATIONS_METRIC} for the matching rule to sum to \
             {expected_invocations}, got {invoked}"
        );
        assert_eq!(
            delivered,
            if row.targeted { row.events } else { 0 },
            "case {case:?}: expected the rule's target to hold every matched event, \
             got {delivered} message(s)"
        );

        // And the series CloudWatch holds: the matched rule's two match series
        // per dimension set, plus its delivery series where it delivered, and
        // nothing at all for the rule that did not match.
        let series = published_series(&emulator).await;
        let mut metrics = MATCH_METRICS.to_vec();
        if row.targeted {
            metrics.push(INVOCATIONS_METRIC);
        }
        assert_eq!(
            ordered(&series),
            ordered(&rule_series(row.bus, MATCHING_RULE, &metrics)),
            "case {case:?}: expected only the matched rule's own series, got {series:?}"
        );
    }
}
