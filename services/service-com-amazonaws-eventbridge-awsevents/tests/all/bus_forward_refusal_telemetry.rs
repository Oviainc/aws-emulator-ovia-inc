// SPDX-License-Identifier: CC0-1.0

//! The AWS/Events telemetry a client reads to tell "the destination bus REFUSED
//! the forward" apart from "nothing was ever published", end to end over the wire
//! through the real `MultiServiceRouter` with SQS, IAM and CloudWatch mounted
//! beside EventBridge — every counter read with `GetMetricStatistics` and
//! `ListMetrics`, as a client watching a cross-bus fan-out reads them.
//!
//! The RULE asserted here, over every round a bus-to-bus forward is withheld in:
//! a rule's pattern match is counted on that rule's own routing counters
//! (`MatchedEvents`, `TriggeredRules`) whatever became of the handover it was then
//! given; a handover a target's own API accepted is counted on that rule's
//! `Invocations` and `SuccessfulInvocationAttempts`; and a counter is published
//! only when there is something to count, so the invocation-accounting counters of
//! a forward onto another event bus — a PUBLICATION onto that bus rather than an
//! invocation of a target's API — that was refused and dead-lettered
//! (`FailedInvocations`, `InvocationsSentToDlq`) hold no series at all: a read of
//! them answers with NO datapoint, which is a different observation from a zero
//! one. The three reads together are what distinguish a mis-scoped destination
//! policy from a producer that stopped publishing.
//!
//! The table spans the class rather than the one recorded case: its rows differ
//! along HOW MANY events the round publishes (so a counter is a count rather than
//! a flag), along whether the withheld forward reaches a dead-letter queue at all
//! (a target carrying a `DeadLetterConfig`, and one carrying none, where the
//! refused event is dropped), and along whether the forward is withheld at all —
//! the last row is the control that shows the same reads report a forward that WAS
//! published. Each row is read BOTH ways a client reads a live series: over the
//! one 60-second period ending now, and over a window around now.
//!
//! The harness (the router with EventBridge, SQS, IAM and CloudWatch on one
//! engine) is the one `target_dispatch` owns; the three rules over the two buses
//! are the ones `bus_forward_authorization` builds, which pins the authorization
//! half of the same forward, and `bus_forward_failure_accounting` the dead letter
//! and the routing counters. What this suite adds is the WHOLE read a cross-bus
//! operator makes: the accepted control delivery's success counter beside the
//! withheld forward's absent ones, including `InvocationsSentToDlq`.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    FAILED_INVOCATIONS_METRIC, INVOCATIONS_METRIC, MATCHED_EVENTS_METRIC, METRIC_NAMESPACE,
    NO_PERMISSIONS, PUT_EVENTS_ACTION, SERVICE_PRINCIPAL, SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC,
    TRIGGERED_RULES_METRIC,
};

use crate::bus_forward_authorization::{
    bus_arn, given_bus, given_rule, CONTROL_RULE, DESTINATION_BUS, FORWARDING_ROLE, FORWARDING_RULE,
    RECEIVING_RULE, SOURCE_BUS, TARGET_ID,
};
use crate::target_dispatch::{
    cloudwatch, drain, emulator, events, given_queue, given_role, published_series, rule_dimensions,
    rule_name_dimensions, Emulator, DETAIL_TYPE, SOURCE,
};

/// The AWS/Events counter a client reads to see how many events EventBridge sent
/// to a target's dead-letter queue.
///
/// Named here rather than taken from the service: the AWS/Events metric
/// vocabulary is in no Smithy model, so this is the name the read asks for, and a
/// constant the service owned would move with a regression instead of catching
/// it.
const INVOCATIONS_SENT_TO_DLQ_METRIC: &str = "InvocationsSentToDlq";

/// The `StandardUnit` a count of events is published in.
const COUNT_UNIT: &str = "Count";

/// The member of an event envelope carrying the event's id, as a dead letter and
/// a delivered copy each carry the event they are about.
const EVENT_ID_FIELD: &str = "id";

/// One way a client scopes a read of a live series: the window it gives
/// `GetMetricStatistics` relative to now, and the period it folds that window
/// into.
struct Read {
    described: &'static str,
    start: f64,
    end: f64,
    period: i64,
}

/// Both ways a client reads a counter of a round that has just happened: the one
/// aligned period ending now, and a window around now read at the same period.
const READS: [Read; 2] = [
    Read {
        described: "the 60-second period ending now",
        start: -60.0,
        end: 0.0,
        period: 60,
    },
    Read {
        described: "a window around now",
        start: -300.0,
        end: 300.0,
        period: 60,
    },
];

/// Seconds since the epoch, as a read's `StartTime` / `EndTime` carry it.
fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The datapoints `GetMetricStatistics` answers for `metric` over the ONE series
/// `dimensions` names, scoped the way `read` says.
async fn datapoints(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    dimensions: &Value,
    read: &Read,
) -> Vec<Value> {
    let now = epoch_seconds();
    let (status, stats) = cloudwatch(
        emulator,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": metric,
            "Dimensions": dimensions,
            "StartTime": now + read.start,
            "EndTime": now + read.end,
            "Period": read.period,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics for {metric} over {} answered {status} {stats}",
        read.described
    );
    assert_eq!(
        stats["Label"],
        json!(metric),
        "case {case:?}: expected the read to be labelled with the metric, got {stats}"
    );
    stats["Datapoints"].as_array().cloned().unwrap_or_default()
}

/// The `Sum` the datapoints `points` hold together.
fn summed(points: &[Value]) -> f64 {
    points
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// Assert the series `dimensions` names holds datapoints summing to `want`, in
/// `Count`, read `read`'s way.
async fn assert_counted(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    whose: &str,
    dimensions: &Value,
    want: f64,
    read: &Read,
) {
    let points = datapoints(emulator, case, metric, dimensions, read).await;
    assert!(
        !points.is_empty(),
        "case {case:?}: expected {metric} for {whose} to hold a datapoint over {}, got none",
        read.described
    );
    let got = summed(&points);
    assert_eq!(
        got, want,
        "case {case:?}: expected {metric} for {whose} to sum to {want} over {}, got {got} \
         from {points:?}",
        read.described
    );
    for point in &points {
        assert_eq!(
            point["Unit"],
            json!(COUNT_UNIT),
            "case {case:?}: expected every {metric} datapoint for {whose} in {COUNT_UNIT}, \
             got {point}"
        );
    }
}

/// Assert the series `dimensions` names was never published: a read of it answers
/// with no datapoint at all rather than with a zero one.
async fn assert_unpublished(
    emulator: &Emulator,
    case: &str,
    metric: &str,
    whose: &str,
    dimensions: &Value,
    read: &Read,
) {
    let points = datapoints(emulator, case, metric, dimensions, read).await;
    assert!(
        points.is_empty(),
        "case {case:?}: expected no {metric} datapoint for {whose} over {}, got {points:?}",
        read.described
    );
}

/// How many rounds of receiving a queue is drained over, so a round that answers
/// fewer messages than are waiting does not decide an assertion.
const DRAIN_ROUNDS: usize = 8;

/// Every message waiting on the queue at `url`, over several receives.
async fn drain_all(emulator: &Emulator, url: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    for _ in 0..DRAIN_ROUNDS {
        let received = drain(emulator, url).await;
        if received.is_empty() {
            break;
        }
        messages.extend(received);
    }
    messages
}

/// The event ids the messages `messages` carry in their bodies, sorted, so a set
/// of deliveries is compared as the set it is.
fn carried_event_ids(messages: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = messages
        .iter()
        .filter_map(|message| {
            let body = message["Body"].as_str()?;
            let envelope: Value = serde_json::from_str(body).ok()?;
            envelope[EVENT_ID_FIELD].as_str().map(str::to_string)
        })
        .collect();
    ids.sort();
    ids
}

/// `ids` sorted, so a set of event ids is compared as the set it is.
fn sorted(ids: &[String]) -> Vec<String> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    sorted
}

/// Deny the forwarding role the publication onto the destination bus, and read
/// the document back through the implemented reader, so what the forward is
/// decided against is what a client can see on the bus.
async fn given_destination_deny(emulator: &Emulator, case: &str, role_arn: &str) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "withholds-the-forward",
            "Effect": "Deny",
            "Principal": { "AWS": role_arn },
            "Action": PUT_EVENTS_ACTION,
            "Resource": bus_arn(DESTINATION_BUS),
        }],
    })
    .to_string();
    let (status, body) = events(
        emulator,
        "PutPermission",
        &json!({ "EventBusName": DESTINATION_BUS, "Policy": document }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutPermission answered {status} {body}"
    );
    let (status, described) = events(
        emulator,
        "DescribeEventBus",
        &json!({ "Name": DESTINATION_BUS }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeEventBus answered {status} {described}"
    );
    let reported = described["Policy"].as_str().unwrap_or_default();
    assert!(
        reported.contains(PUT_EVENTS_ACTION),
        "case {case:?}: expected the installed statement on the bus, got {reported:?}"
    );
}

/// Publish `count` events onto the source bus, each carrying its own nonce, and
/// answer their `EventId`s.
async fn publish_round(emulator: &Emulator, case: &str, count: usize) -> Vec<String> {
    let entries: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "EventBusName": SOURCE_BUS,
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "nonce": format!("n-{index}") }).to_string(),
            })
        })
        .collect();
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": entries })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
    let published: Vec<String> = body["Entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["EventId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no EventId in {entry}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        published.len(),
        count,
        "case {case:?}: expected an EventId per entry, got {published:?}"
    );
    published
}

/// One row: how many events the round publishes, whether the destination bus's
/// policy withholds the forward, and whether the forwarding target carries a
/// dead-letter queue for what is withheld.
struct Row {
    case: &'static str,
    events: usize,
    destination_denies: bool,
    dead_letters: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one event, the destination policy denies the forward, dead letters configured",
            events: 1,
            destination_denies: true,
            dead_letters: true,
        },
        Row {
            case: "three events, the destination policy denies the forward, dead letters \
                   configured",
            events: 3,
            destination_denies: true,
            dead_letters: true,
        },
        Row {
            case: "two events, the destination policy denies the forward, no dead-letter queue \
                   on the target",
            events: 2,
            destination_denies: true,
            dead_letters: false,
        },
        Row {
            case: "one event, nothing withholds the forward",
            events: 1,
            destination_denies: false,
            dead_letters: true,
        },
    ]
}

impl Row {
    /// Whether this row's forward is withheld: the destination bus's own policy
    /// is what decides it, and the dead-letter queue is only where a withheld
    /// event is written when the target names one.
    fn withheld(&self) -> bool {
        self.destination_denies
    }
}

#[tokio::test]
async fn the_events_counters_tell_a_refused_forward_from_an_unpublished_event() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, SOURCE_BUS).await;
        given_bus(&emulator, case, DESTINATION_BUS).await;
        let (consumer_url, consumer_arn) = given_queue(&emulator, "destination-consumer").await;
        let (control_url, control_arn) = given_queue(&emulator, "control").await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;
        // The forwarding role's OWN policy allows the publication in every row,
        // so what withholds a forward here is the destination bus's policy alone.
        let role_arn = given_role(
            &emulator,
            FORWARDING_ROLE,
            SERVICE_PRINCIPAL,
            Some(PUT_EVENTS_ACTION),
        )
        .await;

        // The rule on the destination bus, which delivers a forwarded copy to its
        // own queue; the twin control on the source bus, whose queue no policy
        // withholds; and the forwarding rule itself.
        given_rule(
            &emulator,
            case,
            DESTINATION_BUS,
            RECEIVING_RULE,
            json!([{ "Id": TARGET_ID, "Arn": consumer_arn }]),
        )
        .await;
        given_rule(
            &emulator,
            case,
            SOURCE_BUS,
            CONTROL_RULE,
            json!([{ "Id": TARGET_ID, "Arn": control_arn }]),
        )
        .await;
        let destination_arn = bus_arn(DESTINATION_BUS);
        let mut forwarding_target = json!({
            "Id": TARGET_ID,
            "Arn": destination_arn,
            "RoleArn": role_arn,
        });
        if row.dead_letters {
            forwarding_target["DeadLetterConfig"] = json!({ "Arn": dead_letter_arn });
        }
        given_rule(
            &emulator,
            case,
            SOURCE_BUS,
            FORWARDING_RULE,
            json!([forwarding_target]),
        )
        .await;
        if row.destination_denies {
            given_destination_deny(&emulator, case, &role_arn).await;
        }

        let published = publish_round(&emulator, case, row.events).await;
        let counted = f64::from(u32::try_from(row.events).unwrap_or(u32::MAX));

        // The twin control: every event this round published reached the source
        // bus's rules, so an absence at the destination is what withheld the
        // forward rather than the publication not having happened.
        let control = drain_all(&emulator, &control_url).await;
        assert_eq!(
            carried_event_ids(&control),
            sorted(&published),
            "case {case:?}: expected the control rule's queue to hold every published event, \
             got {control:?}"
        );

        // What the destination bus's own rule delivered: a copy per event only
        // when the forward went through.
        let forwarded = drain_all(&emulator, &consumer_url).await;
        let expected_copies = if row.withheld() { 0 } else { row.events };
        assert_eq!(
            forwarded.len(),
            expected_copies,
            "case {case:?}: expected {expected_copies} copy/copies on the destination bus's \
             consumer queue, got {forwarded:?}"
        );

        // What reached the dead-letter queue: the withheld event itself, carrying
        // the condition, where the forwarding target has one to write to.
        let dead_letters = drain_all(&emulator, &dead_letter_url).await;
        let expected_dead_letters = if row.withheld() && row.dead_letters {
            sorted(&published)
        } else {
            Vec::new()
        };
        assert_eq!(
            carried_event_ids(&dead_letters),
            expected_dead_letters,
            "case {case:?}: expected the dead-letter queue to hold {expected_dead_letters:?}, \
             got {dead_letters:?}"
        );
        for message in &dead_letters {
            assert_eq!(
                message["MessageAttributes"]["ERROR_CODE"]["StringValue"],
                json!(NO_PERMISSIONS),
                "case {case:?}: expected the dead-letter ERROR_CODE {NO_PERMISSIONS}, got {message}"
            );
        }

        for read in &READS {
            // The forwarding rule TRIGGERED on every event its pattern matched,
            // whatever came of the handover it was then given: this is the read
            // that says "the rule matched and the delivery was refused", so it
            // answers a datapoint however the round ended.
            for metric in [MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC] {
                assert_counted(
                    &emulator,
                    case,
                    metric,
                    "the forwarding rule",
                    &rule_dimensions(SOURCE_BUS, FORWARDING_RULE),
                    counted,
                    read,
                )
                .await;
            }

            // The twin control rule's own handover was ACCEPTED by its queue, so
            // the same namespace reports it as a delivery that went through —
            // read under both dimension sets a handover counter is published
            // under.
            for metric in [INVOCATIONS_METRIC, SUCCESSFUL_INVOCATION_ATTEMPTS_METRIC] {
                for (dimensioned, dimensions) in [
                    (
                        "the control rule under the bus and the rule",
                        rule_dimensions(SOURCE_BUS, CONTROL_RULE),
                    ),
                    (
                        "the control rule under the rule name alone",
                        rule_name_dimensions(CONTROL_RULE),
                    ),
                ] {
                    assert_counted(
                        &emulator,
                        case,
                        metric,
                        dimensioned,
                        &dimensions,
                        counted,
                        read,
                    )
                    .await;
                }
            }

            // A forward is a publication onto the other bus rather than an
            // invocation of a target's own API, so neither the failure counter
            // nor the dead-letter counter is published for it — in either
            // dimension set, and whether or not the event reached a dead-letter
            // queue. A read answers with NO datapoint, which is what tells an
            // intentionally-withheld zero from a store that publishes nothing.
            for metric in [FAILED_INVOCATIONS_METRIC, INVOCATIONS_SENT_TO_DLQ_METRIC] {
                for (dimensioned, dimensions) in [
                    (
                        "the forwarding rule under the bus and the rule",
                        rule_dimensions(SOURCE_BUS, FORWARDING_RULE),
                    ),
                    (
                        "the forwarding rule under the rule name alone",
                        rule_name_dimensions(FORWARDING_RULE),
                    ),
                ] {
                    assert_unpublished(&emulator, case, metric, dimensioned, &dimensions, read)
                        .await;
                }
            }
        }

        // And the namespace itself holds no such series at all: every delivery
        // this round made was accepted, so there is nothing for either counter to
        // count anywhere.
        let series = published_series(&emulator).await;
        for metric in [FAILED_INVOCATIONS_METRIC, INVOCATIONS_SENT_TO_DLQ_METRIC] {
            assert!(
                !series
                    .iter()
                    .any(|entry| entry["MetricName"] == json!(metric)),
                "case {case:?}: expected no {metric} series in EventBridge's namespace, \
                 got {series:?}"
            );
        }
    }
}
