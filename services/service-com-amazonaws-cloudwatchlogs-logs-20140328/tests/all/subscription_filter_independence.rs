// SPDX-License-Identifier: CC0-1.0

//! Each subscription filter of ONE log group as its OWN subscription, end to end over the
//! wire with the Kinesis emulator (the destinations) and the CloudWatch emulator (the metric
//! store) mounted beside this service on one engine.
//!
//! RULE: a log group's subscription filters are independent subscriptions over the same
//! ingested batch — every filter is offered every stored event, its own `filterPattern`
//! selects out of the reading its own `applyOnTransformedLogs` chooses ("the subscription
//! filter will be applied on the transformed version of the log events instead of the
//! original ingested log events"), the envelope it is handed names that filter alone in
//! `subscriptionFilters` and carries the reading its pattern was applied to, and its delivery
//! is reported under its own `FilterName` dimension — so one filter's outcome, including a
//! destination that would not take its envelope, never changes a sibling's.
//!
//! The suites beside this one grade one filter at a time: `subscription_fan_out.rs` the shape
//! of a single envelope, `transformed_subscription_delivery.rs` the reading ONE filter is
//! applied to (one filter per log group, one group per row), `subscription_delivery_telemetry.rs`
//! the dimensions and counts of ONE filter's series, and `subscription_filter_delivery.rs`
//! WHICH filters a batch is fanned out through (all of them configured alike). What no suite
//! grades is the filters of one group differing from each other: this one does, and its rows
//! differ along what the rule generalises over — the form of the pattern, which events it
//! selects, and the reading (ingested or transformed) each filter sees — and then along what
//! happens to a sibling when one filter's destination disappears.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::harness::{base_ms, now_ms};
use crate::subscription_delivery_telemetry::{
    destination_type_of, dimensions, listed_series, sum, DELIVERY_ERRORS, DESTINATION_TYPE,
    FILTER_NAME, FORWARDED_LOG_EVENTS, LOG_GROUP_NAME,
};
use crate::subscription_fan_out::{past_activation, role_arn, stream_arn, Emulator};
use crate::subscription_filter_delivery::delivered_by;
use crate::transformed_subscription_delivery::{
    full_record_document, parse_vpc, rejected_record_document, FULL_RECORD, REJECTED_RECORD,
};

/// The log stream both tests write to.
const LOG_STREAM: &str = "instance-one";

/// One subscription filter of the shared log group.
struct Subscription {
    /// What this filter varies, named in every assertion about it.
    case: &'static str,
    /// Its `filterName`, which is also the `FilterName` dimension value of its own series.
    name: &'static str,
    /// Its `filterPattern`.
    pattern: &'static str,
    /// Its `applyOnTransformedLogs`, which decides WHICH reading of each event its pattern
    /// runs against and its envelope carries.
    apply_on_transformed_logs: bool,
    /// Which of the batch's events its pattern selects, as indices into [`written`].
    selects: &'static [usize],
}

/// The batch every filter of the first test is offered: one VPC flow log record the interface
/// accepted and one it rejected, each paired with the document the group's `parseVPC`
/// transformer reads it as.
fn written() -> Vec<(&'static str, Value)> {
    vec![
        (FULL_RECORD, full_record_document()),
        (REJECTED_RECORD, rejected_record_document()),
    ]
}

/// The filters the shared group holds: the attached-filter quota's worth, no two alike.
fn subscriptions() -> Vec<Subscription> {
    vec![
        Subscription {
            case: "everything, as ingested",
            name: "raw-everything",
            pattern: "",
            apply_on_transformed_logs: false,
            selects: &[0, 1],
        },
        Subscription {
            case: "everything, transformed",
            name: "transformed-everything",
            pattern: "",
            apply_on_transformed_logs: true,
            selects: &[0, 1],
        },
        Subscription {
            // A JSON condition can only select a JSON event, and the ingested record is not
            // JSON at all — so this filter selecting anything is the transformed reading
            // being what its pattern ran against.
            case: "a JSON condition on the transformed event",
            name: "transformed-rejects",
            pattern: "{$.action = \"REJECT\"}",
            apply_on_transformed_logs: true,
            selects: &[1],
        },
        Subscription {
            case: "an unstructured term on the ingested text",
            name: "raw-accepts",
            pattern: "ACCEPT",
            apply_on_transformed_logs: false,
            selects: &[0],
        },
        Subscription {
            case: "a JSON condition selecting the other transformed event",
            name: "transformed-accepts",
            pattern: "{$.action = \"ACCEPT\"}",
            apply_on_transformed_logs: true,
            selects: &[0],
        },
    ]
}

/// A delivered message as the reading it is: the JSON document a transformed event carries,
/// or the text an event carries as it was ingested — which is not JSON, so the two readings
/// of one event can never be confused for each other.
fn reading(message: &str) -> Value {
    ::serde_json::from_str::<Value>(message)
        .unwrap_or_else(|_| Value::String(message.to_string()))
}

/// The readings each filter's envelopes carried, keyed on the filter the envelope named.
fn carried_by_filter(case: &str, records: &[(String, Value)]) -> BTreeMap<String, Vec<Value>> {
    let mut carried: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (filter, messages) in delivered_by(case, records) {
        carried
            .entry(filter)
            .or_default()
            .extend(messages.iter().map(|message| reading(message)));
    }
    carried
}

/// One `PutSubscriptionFilter` body against `group`.
fn put_filter(group: &str, name: &str, pattern: &str, destination: &str, transformed: bool) -> Value {
    json!({
        "logGroupName": group,
        "filterName": name,
        "filterPattern": pattern,
        "destinationArn": destination,
        "roleArn": role_arn(),
        "applyOnTransformedLogs": transformed,
    })
}

/// The log group and log stream a test writes into, with `transformer` put on the group where
/// it names one.
async fn given_log_stream(emulator: &Emulator, case: &str, group: &str, transformer: Option<Value>) {
    emulator
        .logs(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    emulator
        .logs(
            case,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
        )
        .await;
    if let Some(transformer) = transformer {
        emulator
            .logs(
                case,
                "PutTransformer",
                &json!({"logGroupIdentifier": group, "transformerConfig": transformer}),
            )
            .await;
    }
}

/// One batch of `(timestamp, message)` pairs.
async fn write(emulator: &Emulator, case: &str, group: &str, events: &[(i64, &str)]) {
    emulator
        .logs(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": LOG_STREAM,
                "logEvents": crate::harness::batch(events),
            }),
        )
        .await;
}

/// The messages a log group's stream holds, in the order `GetLogEvents` reports them.
async fn stored(emulator: &Emulator, case: &str, group: &str) -> Vec<String> {
    let page = emulator
        .logs(
            case,
            "GetLogEvents",
            &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
        )
        .await;
    crate::harness::messages(&page)
}

/// Every filter of one log group carries the reading ITS pattern selected on, under its own
/// name, and counts its own forwarded events — while the group stores the ingested bytes.
#[tokio::test]
async fn every_filter_of_one_group_carries_the_reading_its_own_pattern_selected() {
    let case = "five differing filters on one transformed group";
    let group = "/aws/emulator/independent-filters";
    let emulator = Emulator::new();
    let stream = "independent-filters";
    emulator.create_stream(case, stream).await;
    tokio::time::sleep(past_activation()).await;
    given_log_stream(&emulator, case, group, Some(parse_vpc())).await;

    let subscriptions = subscriptions();
    for subscription in &subscriptions {
        emulator
            .logs(
                subscription.case,
                "PutSubscriptionFilter",
                &put_filter(
                    group,
                    subscription.name,
                    subscription.pattern,
                    &stream_arn(stream),
                    subscription.apply_on_transformed_logs,
                ),
            )
            .await;
    }

    let written = written();
    let at = base_ms();
    let events: Vec<(i64, &str)> = written
        .iter()
        .enumerate()
        .map(|(nth, (message, _))| (at + nth as i64, *message))
        .collect();
    write(&emulator, case, group, &events).await;

    // The envelopes, attributed to the filter each one names.
    let records = emulator.delivered(case, stream).await;
    let carried = carried_by_filter(case, &records);
    for (_key, envelope) in &records {
        for (member, expected) in [
            ("owner", ::engine::emulator_account_id().to_string()),
            ("logGroup", group.to_string()),
            ("logStream", LOG_STREAM.to_string()),
        ] {
            assert_eq!(
                envelope[member].as_str(),
                Some(expected.as_str()),
                "case {case}: every envelope names the {member} the events were ingested \
                 into, got {envelope}"
            );
        }
    }
    for subscription in &subscriptions {
        let expected: Vec<Value> = subscription
            .selects
            .iter()
            .map(|index| {
                let (ingested, transformed) = &written[*index];
                if subscription.apply_on_transformed_logs {
                    transformed.clone()
                } else {
                    Value::String((*ingested).to_string())
                }
            })
            .collect();
        let actual = carried.get(subscription.name).cloned().unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {}: the filter carries the events its pattern selected, in the reading it \
             was applied to, got {actual:?}",
            subscription.case
        );
    }
    let expected_stamps: BTreeMap<String, Vec<i64>> = subscriptions
        .iter()
        .map(|subscription| {
            (
                subscription.name.to_string(),
                subscription
                    .selects
                    .iter()
                    .map(|index| at + *index as i64)
                    .collect(),
            )
        })
        .collect();
    let mut delivered_stamps: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (_key, envelope) in &records {
        let filter = envelope["subscriptionFilters"][0]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: an envelope names its filter, got {envelope}"))
            .to_string();
        let stamps = delivered_stamps.entry(filter).or_default();
        for event in envelope["logEvents"].as_array().cloned().unwrap_or_default() {
            stamps.push(event["timestamp"].as_i64().unwrap_or_else(|| {
                panic!("case {case}: a delivered event carries its timestamp, got {event}")
            }));
        }
    }
    assert_eq!(
        delivered_stamps, expected_stamps,
        "case {case}: a transformed reading keeps the instant its event was ingested with, \
         got {delivered_stamps:?}"
    );

    // Transforming for a delivery changes nothing the group stored.
    let held = stored(&emulator, case, group).await;
    assert_eq!(
        held,
        written
            .iter()
            .map(|(message, _)| (*message).to_string())
            .collect::<Vec<String>>(),
        "case {case}: the group holds the ingested bytes, got {held:?}"
    );

    // One series per filter that forwarded anything, each under exactly the three dimension
    // names of a log-group-level filter, and each summing only its own forwarded events.
    let names: BTreeSet<String> = [LOG_GROUP_NAME, DESTINATION_TYPE, FILTER_NAME]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut series = listed_series(&emulator, case, FORWARDED_LOG_EVENTS, group).await;
    series.sort_by(|left, right| left.1.cmp(&right.1));
    let expected_series: Vec<(BTreeSet<String>, String)> = subscriptions
        .iter()
        .filter(|subscription| !subscription.selects.is_empty())
        .map(|subscription| (names.clone(), subscription.name.to_string()))
        .collect();
    let mut expected_series = expected_series;
    expected_series.sort_by(|left, right| left.1.cmp(&right.1));
    assert_eq!(
        series, expected_series,
        "case {case}: each filter publishes its own series under its own name, got {series:?}"
    );
    let destination_type = destination_type_of(&emulator, case, group).await;
    let now = now_ms();
    for subscription in &subscriptions {
        let reported = sum(
            &emulator,
            subscription.case,
            FORWARDED_LOG_EVENTS,
            &dimensions(group, subscription.name, &destination_type),
            now,
        )
        .await;
        assert_eq!(
            reported,
            Some(subscription.selects.len() as f64),
            "case {}: the filter's own series counts the events ITS pattern selected out of \
             {} written, got {reported:?}",
            subscription.case,
            written.len()
        );
    }
}

/// A destination that would not take one filter's envelope stops THAT filter: it counts the
/// delivery error, stops being attempted until a put re-validates it, and leaves the sibling
/// filter of the same log group delivering every batch.
#[tokio::test]
async fn a_refusing_destination_stops_only_the_filter_that_names_it() {
    let case = "one of two filters loses its destination";
    let group = "/aws/emulator/one-destination-gone";
    let emulator = Emulator::new();
    let standing = "standing-destination";
    let doomed = "doomed-destination";
    emulator.create_stream(case, standing).await;
    emulator.create_stream(case, doomed).await;
    tokio::time::sleep(past_activation()).await;
    given_log_stream(&emulator, case, group, None).await;

    let to_standing = "to-standing";
    let to_doomed = "to-doomed";
    let doomed_put = put_filter(group, to_doomed, "", &stream_arn(doomed), false);
    emulator
        .logs(
            case,
            "PutSubscriptionFilter",
            &put_filter(group, to_standing, "", &stream_arn(standing), false),
        )
        .await;
    emulator
        .logs(case, "PutSubscriptionFilter", &doomed_put)
        .await;

    // Both filters deliver the first batch, so what follows is a change of one destination
    // rather than a filter that never worked.
    let at = base_ms();
    let first = "ERROR while both destinations stand";
    write(&emulator, case, group, &[(at, first)]).await;
    assert_eq!(
        carried_by_filter(case, &emulator.delivered(case, standing).await)
            .get(to_standing)
            .cloned()
            .unwrap_or_default(),
        vec![json!(first)],
        "case {case}: the standing destination holds the first line"
    );
    assert_eq!(
        carried_by_filter(case, &emulator.delivered(case, doomed).await)
            .get(to_doomed)
            .cloned()
            .unwrap_or_default(),
        vec![json!(first)],
        "case {case}: the doomed destination holds the first line too"
    );

    // One destination goes away. Each further batch is offered to both filters.
    emulator
        .kinesis(
            case,
            "DeleteStream",
            &json!({"StreamName": doomed, "EnforceConsumerDeletion": true}),
        )
        .await;
    let during = "ERROR with one destination gone";
    write(&emulator, case, group, &[(at + 10, during)]).await;
    let after = "ERROR once the destination is marked";
    write(&emulator, case, group, &[(at + 20, after)]).await;

    let destination_type = destination_type_of(&emulator, case, group).await;
    let standing_series = dimensions(group, to_standing, &destination_type);
    let doomed_series = dimensions(group, to_doomed, &destination_type);
    let now = now_ms();
    assert_eq!(
        sum(&emulator, case, FORWARDED_LOG_EVENTS, &standing_series, now).await,
        Some(3.0),
        "case {case}: the sibling filter forwards every batch, whatever the other's \
         destination did"
    );
    assert_eq!(
        sum(&emulator, case, DELIVERY_ERRORS, &standing_series, now).await,
        None,
        "case {case}: a filter whose destination took the envelope counts no delivery error"
    );
    assert_eq!(
        carried_by_filter(case, &emulator.delivered(case, standing).await)
            .get(to_standing)
            .cloned()
            .unwrap_or_default(),
        vec![json!(first), json!(during), json!(after)],
        "case {case}: the standing destination holds all three lines"
    );
    // The refusal is counted ONCE: the batch after it is not attempted at all, because the
    // destination stays marked until a put re-validates it.
    assert_eq!(
        sum(&emulator, case, DELIVERY_ERRORS, &doomed_series, now).await,
        Some(1.0),
        "case {case}: the envelope the gone destination would not take counts one \
         {DELIVERY_ERRORS}, and the batch after it is not attempted"
    );
    assert_eq!(
        sum(&emulator, case, FORWARDED_LOG_EVENTS, &doomed_series, now).await,
        Some(1.0),
        "case {case}: the filter whose destination went away forwarded only what it \
         delivered before"
    );

    // Nothing written during the outage is lost: the log group holds every line.
    let held = stored(&emulator, case, group).await;
    assert_eq!(
        held,
        vec![first.to_string(), during.to_string(), after.to_string()],
        "case {case}: every line stays readable in the log group, got {held:?}"
    );

    // The identical put re-validates the destination, so the next batch is ATTEMPTED again —
    // and against a destination that is still gone it counts a second delivery error, while
    // the sibling goes on delivering.
    emulator
        .logs(case, "PutSubscriptionFilter", &doomed_put)
        .await;
    let re_put = "ERROR after the re-put";
    write(&emulator, case, group, &[(at + 30, re_put)]).await;
    let now = now_ms();
    assert_eq!(
        sum(&emulator, case, DELIVERY_ERRORS, &doomed_series, now).await,
        Some(2.0),
        "case {case}: a re-validated destination is attempted again, and refusing again \
         counts another {DELIVERY_ERRORS}"
    );
    assert_eq!(
        sum(&emulator, case, FORWARDED_LOG_EVENTS, &standing_series, now).await,
        Some(4.0),
        "case {case}: the sibling filter forwards the batch after the re-put as well"
    );
}
