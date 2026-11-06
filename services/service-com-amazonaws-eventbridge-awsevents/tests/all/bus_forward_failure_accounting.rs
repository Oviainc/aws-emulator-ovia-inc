// SPDX-License-Identifier: CC0-1.0

//! What a bus-to-bus forward EventBridge could not make is ACCOUNTED as, end to
//! end over the wire through the real `MultiServiceRouter` with SQS, IAM and
//! CloudWatch mounted beside EventBridge.
//!
//! The RULE asserted here, over every way such a forward can be withheld: a
//! rule's match is counted on that rule's own AWS/Events routing counters
//! (`MatchedEvents`, `TriggeredRules`) because its pattern matched, whatever came
//! of the targets it was then handed to; an event EventBridge could not hand to a
//! target is written to that target's `DeadLetterConfig` queue carrying the
//! condition that names the step which failed (`NO_PERMISSIONS` where a policy
//! withholds the call, `FAILED_TO_ASSUME_ROLE` where the role cannot be assumed
//! at all); and a forward onto another event bus is a PUBLICATION rather than an
//! invocation of a target's own API, so nothing is counted on the forwarding
//! rule's `FailedInvocations` for one that was refused — the series is not
//! published at all, which is a different observation from a zero datapoint.
//! Nothing of the destination bus sees a refused forward, so its own rules do not
//! trigger either.
//!
//! The table spans the class rather than the one recorded case: its rows differ
//! along WHICH half of the decision withholds the forward (the destination bus's
//! resource policy denying the forwarding role, the role's own policies saying
//! nothing about the publication, the role being allowed another action of the
//! service instead, the role's trust policy naming another service), along the
//! CONDITION the refusal therefore carries, along HOW MANY events the round
//! publishes (so a counter is a count rather than a flag) and along whether the
//! forward is withheld at all — a row nothing withholds is the control that shows
//! the same accounting records a forward that WAS published.
//!
//! The harness (the router with EventBridge, SQS, IAM and CloudWatch on one
//! engine) is the one `target_dispatch` owns; the three rules over the two buses
//! are the ones `bus_forward_authorization` builds, which pins the authorization
//! half of the same forward.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    InvocationOutcome, FAILED_INVOCATIONS_METRIC, FAILED_TO_ASSUME_ROLE, MATCHED_EVENTS_METRIC,
    NO_PERMISSIONS, PUT_EVENTS_ACTION, SERVICE_PRINCIPAL, TRIGGERED_RULES_METRIC,
};

use crate::bus_forward_authorization::{
    bus_arn, given_bus, given_rule, rule_arn, CONTROL_RULE, DESTINATION_BUS, FORWARDING_ROLE,
    FORWARDING_RULE, RECEIVING_RULE, SOURCE_BUS, TARGET_ID,
};
use crate::target_dispatch::{
    drain, emulator, events, given_queue, given_role, invocations, metric_sum, metric_sum_over,
    published_series, rule_name_dimensions, Emulator, DETAIL_TYPE, SOURCE,
};

/// The service principal of a service OTHER than this one, for the row whose
/// forwarding role does not trust EventBridge: a role whose trust policy names it
/// cannot be assumed to make the forward at all.
const ANOTHER_SERVICE_PRINCIPAL: &str = "s3.amazonaws.com";

/// An action of this service other than the publication a forward makes, for the
/// row whose role is allowed something else.
const ANOTHER_EVENTS_ACTION: &str = "events:PutRule";

/// The member of an event envelope carrying the event's id, as a dead letter
/// carries the event it could not deliver.
const EVENT_ID_FIELD: &str = "id";

/// What the row expects of the handover to the event-bus target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The copy was published onto the destination bus.
    Forwarded,
    /// The forward was withheld, and the dead letter carries this condition.
    Refused(&'static str),
}

/// One row: how the forward is (or is not) withheld, how many events the round
/// publishes, and what the accounting must then show.
struct Row {
    case: &'static str,
    /// The service principal the forwarding role's trust policy names.
    trusted: &'static str,
    /// The one action the role's inline policy allows — `None` for a role
    /// carrying no inline policy at all.
    allowed: Option<&'static str>,
    /// Whether the DESTINATION bus's resource policy denies that role the
    /// publication.
    destination_denies: bool,
    events: usize,
    expected: Expected,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the destination bus's policy denies the forwarding role the publication",
            trusted: SERVICE_PRINCIPAL,
            allowed: Some(PUT_EVENTS_ACTION),
            destination_denies: true,
            events: 1,
            expected: Expected::Refused(NO_PERMISSIONS),
        },
        Row {
            case: "a denied forward of a round carrying three events",
            trusted: SERVICE_PRINCIPAL,
            allowed: Some(PUT_EVENTS_ACTION),
            destination_denies: true,
            events: 3,
            expected: Expected::Refused(NO_PERMISSIONS),
        },
        Row {
            case: "a forwarding role carrying no identity policy at all",
            trusted: SERVICE_PRINCIPAL,
            allowed: None,
            destination_denies: false,
            events: 1,
            expected: Expected::Refused(NO_PERMISSIONS),
        },
        Row {
            case: "a forwarding role allowed another action of the service instead",
            trusted: SERVICE_PRINCIPAL,
            allowed: Some(ANOTHER_EVENTS_ACTION),
            destination_denies: false,
            events: 2,
            expected: Expected::Refused(NO_PERMISSIONS),
        },
        Row {
            case: "a forwarding role whose trust policy names another service",
            trusted: ANOTHER_SERVICE_PRINCIPAL,
            allowed: Some(PUT_EVENTS_ACTION),
            destination_denies: false,
            events: 1,
            expected: Expected::Refused(FAILED_TO_ASSUME_ROLE),
        },
        Row {
            case: "a forward nothing withholds",
            trusted: SERVICE_PRINCIPAL,
            allowed: Some(PUT_EVENTS_ACTION),
            destination_denies: false,
            events: 2,
            expected: Expected::Forwarded,
        },
    ]
}

/// Deny the forwarding role the publication onto the destination bus, through a
/// whole policy document, and read it back through the implemented reader so what
/// the forward is decided against is what a client can see on the bus.
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

/// How many rounds of receiving a queue is drained over, so a round that answers
/// fewer messages than are waiting does not decide the assertion.
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

/// `ids` sorted, so a set of event ids is compared as the set it is.
fn sorted(ids: &[String]) -> Vec<String> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    sorted
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

#[tokio::test]
async fn a_withheld_bus_forward_is_dead_lettered_and_counts_no_failed_invocation() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        given_bus(&emulator, case, SOURCE_BUS).await;
        given_bus(&emulator, case, DESTINATION_BUS).await;
        let (consumer_url, consumer_arn) = given_queue(&emulator, "destination-consumer").await;
        let (control_url, control_arn) = given_queue(&emulator, "control").await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;
        let role_arn = given_role(&emulator, FORWARDING_ROLE, row.trusted, row.allowed).await;

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
        given_rule(
            &emulator,
            case,
            SOURCE_BUS,
            FORWARDING_RULE,
            json!([{
                "Id": TARGET_ID,
                "Arn": destination_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }]),
        )
        .await;
        if row.destination_denies {
            given_destination_deny(&emulator, case, &role_arn).await;
        }

        let published = publish_round(&emulator, case, row.events).await;

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
        let expected_copies = match row.expected {
            Expected::Forwarded => row.events,
            Expected::Refused(_) => 0,
        };
        assert_eq!(
            forwarded.len(),
            expected_copies,
            "case {case:?}: expected {expected_copies} copy/copies on the destination bus's \
             consumer queue, got {forwarded:?}"
        );

        // What EventBridge recorded about each handover to the bus target.
        let recorded = invocations(&emulator);
        let handovers: Vec<&InvocationOutcome> = recorded
            .iter()
            .filter(|(rule, target, _)| rule == FORWARDING_RULE && *target == destination_arn)
            .map(|(_, _, outcome)| outcome)
            .collect();
        assert_eq!(
            handovers.len(),
            row.events,
            "case {case:?}: expected one recorded handover per event, got {recorded:?}"
        );
        let dead_letters = drain_all(&emulator, &dead_letter_url).await;
        match row.expected {
            Expected::Forwarded => {
                for outcome in &handovers {
                    assert_eq!(
                        **outcome,
                        InvocationOutcome::Invoked,
                        "case {case:?}: expected the forward to go through, got {outcome:?}"
                    );
                }
                assert!(
                    dead_letters.is_empty(),
                    "case {case:?}: a forward that was published is not dead-lettered, \
                     got {dead_letters:?}"
                );
            }
            Expected::Refused(condition) => {
                for outcome in &handovers {
                    let InvocationOutcome::Failed { code, message } = outcome else {
                        panic!("case {case:?}: expected a withheld forward, got {outcome:?}");
                    };
                    assert_eq!(
                        code, condition,
                        "case {case:?}: expected the condition {condition}, got {outcome:?}"
                    );
                    let names = if condition == FAILED_TO_ASSUME_ROLE {
                        message.contains(&role_arn)
                    } else {
                        message.contains(PUT_EVENTS_ACTION) && message.contains(&destination_arn)
                    };
                    assert!(
                        names,
                        "case {case:?}: expected the refusal to name what it was about, \
                         got {message:?}"
                    );
                }
                // The ONE signal the owner of a mis-scoped destination gets: the
                // event itself on the forwarding target's dead-letter queue,
                // carrying the condition, the rule and the target.
                assert_eq!(
                    carried_event_ids(&dead_letters),
                    sorted(&published),
                    "case {case:?}: expected every withheld event on the dead-letter queue, \
                     got {dead_letters:?}"
                );
                for message in &dead_letters {
                    let attributes = &message["MessageAttributes"];
                    assert_eq!(
                        attributes["ERROR_CODE"]["StringValue"],
                        json!(condition),
                        "case {case:?}: expected the dead-letter ERROR_CODE {condition}, \
                         got {message}"
                    );
                    assert_eq!(
                        attributes["TARGET_ARN"]["StringValue"],
                        json!(destination_arn),
                        "case {case:?}: the dead letter names the bus target, got {message}"
                    );
                    assert_eq!(
                        attributes["RULE_ARN"]["StringValue"],
                        json!(rule_arn(SOURCE_BUS, FORWARDING_RULE)),
                        "case {case:?}: the dead letter names the forwarding rule, got {message}"
                    );
                    assert!(
                        message["MessageAttributes"]["ERROR_MESSAGE"]["StringValue"]
                            .as_str()
                            .is_some_and(|text| !text.is_empty()),
                        "case {case:?}: the dead letter carries the refusal itself, got {message}"
                    );
                }
            }
        }

        // The forwarding rule TRIGGERED on every event its pattern matched,
        // whatever came of the handover it was then given.
        for metric in [MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC] {
            let sum = metric_sum(&emulator, metric, SOURCE_BUS, FORWARDING_RULE).await;
            let expected = row.events as f64;
            assert_eq!(
                sum, expected,
                "case {case:?}: expected {metric} of {expected} for the forwarding rule, got {sum}"
            );
        }

        // The destination bus's own rule triggered only on the copies that were
        // published onto it: a withheld forward reaches none of its rules.
        for metric in [MATCHED_EVENTS_METRIC, TRIGGERED_RULES_METRIC] {
            let sum = metric_sum(&emulator, metric, DESTINATION_BUS, RECEIVING_RULE).await;
            let expected = expected_copies as f64;
            assert_eq!(
                sum, expected,
                "case {case:?}: expected {metric} of {expected} for the receiving rule, got {sum}"
            );
        }

        // A forward is a publication onto the other bus rather than an invocation
        // of a target's own API, so one that was withheld is counted on the
        // forwarding rule's FailedInvocations in neither dimension set — and, the
        // counter being published only when there is something to count, the
        // namespace holds no such series at all.
        let failed = metric_sum(
            &emulator,
            FAILED_INVOCATIONS_METRIC,
            SOURCE_BUS,
            FORWARDING_RULE,
        )
        .await;
        assert_eq!(
            failed, 0.0,
            "case {case:?}: expected no failed invocation counted for the forwarding rule, \
             got {failed}"
        );
        let by_rule_name = metric_sum_over(
            &emulator,
            FAILED_INVOCATIONS_METRIC,
            &rule_name_dimensions(FORWARDING_RULE),
        )
        .await;
        assert_eq!(
            by_rule_name, 0.0,
            "case {case:?}: expected no failed invocation under the rule name alone, \
             got {by_rule_name}"
        );
        let series = published_series(&emulator).await;
        assert!(
            !series
                .iter()
                .any(|entry| entry["MetricName"] == json!(FAILED_INVOCATIONS_METRIC)),
            "case {case:?}: expected no FailedInvocations series in EventBridge's namespace, \
             got {series:?}"
        );
    }
}
