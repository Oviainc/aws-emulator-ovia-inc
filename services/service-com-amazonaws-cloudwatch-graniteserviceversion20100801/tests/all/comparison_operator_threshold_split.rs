// SPDX-License-Identifier: CC0-1.0

//! Where each static-threshold comparison SPLITS, and the whole chain one evaluated transition
//! drives — end to end over the wire through the real `MultiServiceRouter` with CloudWatch, SNS
//! and SQS on one engine, so what is asserted is what a client that only publishes and polls
//! sees.
//!
//! The RULE asserted here: **a graded datapoint breaches exactly when the arithmetic relation
//! the alarm's `ComparisonOperator` names holds between that datapoint and `Threshold` ("The
//! arithmetic operation to use when comparing the specified statistic and threshold",
//! `MetricAlarm$ComparisonOperator`) — so the two strict operators leave a datapoint exactly ON
//! the threshold un-breached and the two `OrEqualTo` operators count it as a breach — and the
//! state that verdict produces is observable in FULL: `DescribeAlarms` reports the new
//! `StateValue` with a `StateReason` beginning with the threshold-crossing clause,
//! `DescribeAlarmHistory` records a `StateUpdate` item for the transition and an `Action` item
//! for the action it executed, and the topic configured for the state the alarm moved TO
//! delivers the notification to its subscriber.**
//!
//! The operators come from the model's own value list
//! (`smithy_query(query="shape",shape="ComparisonOperator")`): the four that compare against a
//! NUMBER are the ones a static threshold splits at, while `GreaterThanUpperThreshold`,
//! `LessThanLowerThreshold` and `LessThanLowerOrGreaterThanUpperThreshold` compare against the
//! bounds of an anomaly-detection band rather than a threshold value, which
//! [`crate::anomaly_band_alarm_evaluation`] covers.
//!
//! The rows differ along what the rule generalises over: the operator, and where the published
//! value sits relative to the threshold — under it, exactly on it (each operator's published
//! boundary) and over it. Whichever direction a row ends in, the alarm carries an action for
//! BOTH graded states, so every row asserts a delivery rather than only the `ALARM` rows; and
//! the missing-data treatment substitutes nothing, so neither `ALARM` nor `OK` is reachable
//! without grading the datapoint the row really published.
//!
//! What this adds to [`crate::alarm_evaluation`], whose table already grades five of these
//! twelve operator/position cases: the COMPLETE truth table of the four operators, and the
//! whole chain each verdict drives rather than the `StateValue` alone.
//!
//! The emulator seam, the history reader, the recorded-transition reader and the queue reader
//! are the ones [`crate::actions_disabled_evaluation_delivery`] already owns for exactly this
//! surface; the poll-until-a-state reader is
//! [`crate::composite_rule_over_published_children`]'s and the topic-with-subscriber setup is
//! [`crate::sns_action_topic_policy`]'s — all shared rather than copied.

use serde_json::{json, Value};

use crate::actions_disabled_evaluation_delivery::{
    await_history, cw, described, recorded_previous_state, recorded_state, Mailbox,
};
use crate::composite_rule_over_published_children::await_state;
use crate::harness::{emulator, graded_window_stamp, wait_for_period_room};
use crate::sns_action_topic_policy::given_topic_with_subscriber;

/// The namespace the rows publish into: a customer namespace, so the datapoint goes over the
/// wire the way a client's own measurement does.
const NAMESPACE: &str = "Custom/ThresholdSplit";

/// The metric the rows watch inside it.
const METRIC: &str = "Latency";

/// The threshold every row's alarm is compared against.
const THRESHOLD: f64 = 80.0;

/// How far from the threshold the rows that do NOT sit on it publish.
const DELTA: f64 = 5.0;

/// The period every row's alarm is evaluated on, in seconds: the regular cadence, the
/// resolution `PutMetricData` stores a datapoint at by default.
const PERIOD: f64 = 60.0;

/// How much of the current period window one row needs left to it, in seconds.
///
/// A row stamps its datapoint in the newest window an evaluation grades, and the window an
/// evaluation counts back from is the one it runs in — so the publish and the reads that
/// materialise what it implies are asked to land in the window the row measured against.
const WINDOW_ROOM_SECS: f64 = 15.0;

/// The `StateReason` clause an evaluated metric alarm explains its state with.
const CROSSING_CLAUSE: &str = "Threshold Crossed:";

/// The fixed `StateReason` of an alarm nothing has evaluated yet.
const CREATION_REASON: &str = "Unchecked: Initial alarm creation";

/// The `DescribeAlarms` output member a metric alarm is reported under.
const METRIC_ALARMS: &str = "MetricAlarms";

/// The state an alarm no evaluation has graded yet reports, which every row transitions out of.
const UN_EVALUATED: &str = "INSUFFICIENT_DATA";

/// One static-threshold comparison operator, beside which side of the threshold it counts as a
/// breach.
///
/// The three members are the three places a datapoint can sit relative to a number, so an
/// operator's row here is its whole truth table — and the middle one is the split the two
/// strict operators and the two inclusive ones disagree about.
struct Operator {
    /// The modelled `ComparisonOperator` value.
    name: &'static str,
    /// Whether a datapoint UNDER the threshold breaches.
    under: bool,
    /// Whether a datapoint exactly ON the threshold breaches.
    on: bool,
    /// Whether a datapoint OVER the threshold breaches.
    over: bool,
}

/// Every `ComparisonOperator` the model declares that compares against a threshold VALUE
/// (`smithy_query(query="shape",shape="ComparisonOperator")`).
const OPERATORS: &[Operator] = &[
    Operator {
        name: "GreaterThanThreshold",
        under: false,
        on: false,
        over: true,
    },
    Operator {
        name: "GreaterThanOrEqualToThreshold",
        under: false,
        on: true,
        over: true,
    },
    Operator {
        name: "LessThanThreshold",
        under: true,
        on: false,
        over: false,
    },
    Operator {
        name: "LessThanOrEqualToThreshold",
        under: true,
        on: true,
        over: false,
    },
];

/// Where a row publishes relative to the threshold.
#[derive(Debug, Clone, Copy)]
enum Position {
    /// Below it by [`DELTA`].
    Under,
    /// Exactly on it: the boundary the operators split at.
    On,
    /// Above it by [`DELTA`].
    Over,
}

impl Position {
    /// The value a row in this position publishes.
    fn value(self) -> f64 {
        match self {
            Self::Under => THRESHOLD - DELTA,
            Self::On => THRESHOLD,
            Self::Over => THRESHOLD + DELTA,
        }
    }

    /// How this position reads in an assertion message.
    fn label(self) -> &'static str {
        match self {
            Self::Under => "under the threshold",
            Self::On => "exactly on the threshold",
            Self::Over => "over the threshold",
        }
    }
}

/// The three positions, in the order the rows run.
const POSITIONS: &[Position] = &[Position::Under, Position::On, Position::Over];

impl Operator {
    /// Whether a datapoint in `position` breaches this operator's comparison.
    fn breaches(&self, position: Position) -> bool {
        match position {
            Position::Under => self.under,
            Position::On => self.on,
            Position::Over => self.over,
        }
    }

    /// The state an alarm carrying this operator reaches from ONE datapoint in `position`.
    fn state(&self, position: Position) -> &'static str {
        if self.breaches(position) {
            "ALARM"
        } else {
            "OK"
        }
    }
}

/// A member of a described alarm, as the text a client reads.
fn member(alarm: &Value, name: &str) -> String {
    alarm
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The runtime flavour the booted emulator serves on, so the evaluation clock runs the way it
/// does in production rather than inline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_comparison_splits_at_its_threshold_and_drives_the_whole_transition() {
    for operator in OPERATORS {
        for position in POSITIONS.iter().copied() {
            let case = format!("{} {}", operator.name, position.label());
            let wanted = operator.state(position);

            let emulator = emulator();
            let (topic, queue_url) = given_topic_with_subscriber(&emulator, "threshold-split").await;

            let alarm = "threshold-split";
            cw(
                &emulator,
                "PutMetricAlarm",
                &json!({
                    "AlarmName": alarm,
                    "Namespace": NAMESPACE,
                    "MetricName": METRIC,
                    "Dimensions": [{"Name": "Case", "Value": case}],
                    "Statistic": "Average",
                    "Period": PERIOD,
                    "EvaluationPeriods": 1,
                    "DatapointsToAlarm": 1,
                    "Threshold": THRESHOLD,
                    "ComparisonOperator": operator.name,
                    // Substitutes no datapoint for an empty window, so neither ALARM nor OK is
                    // reachable without grading the datapoint this row publishes.
                    "TreatMissingData": "missing",
                    // An action for BOTH graded states, so every row asserts a delivery.
                    "AlarmActions": [topic.clone()],
                    "OKActions": [topic.clone()],
                }),
            )
            .await;

            // Nothing has evaluated the alarm yet, so it reports the fixed creation reason.
            let created = described(&emulator, alarm).await;
            assert_eq!(
                member(&created, "StateValue"),
                UN_EVALUATED,
                "case {case:?}: a new alarm is created un-evaluated, got {created}"
            );
            assert_eq!(
                member(&created, "StateReason"),
                CREATION_REASON,
                "case {case:?}: expected the fixed creation reason {CREATION_REASON:?}, got {:?}",
                member(&created, "StateReason")
            );

            // One datapoint, in the newest window an evaluation grades.
            wait_for_period_room(PERIOD, WINDOW_ROOM_SECS).await;
            cw(
                &emulator,
                "PutMetricData",
                &json!({
                    "Namespace": NAMESPACE,
                    "MetricData": [{
                        "MetricName": METRIC,
                        "Dimensions": [{"Name": "Case", "Value": case}],
                        "Value": position.value(),
                        "Timestamp": graded_window_stamp(PERIOD),
                    }],
                }),
            )
            .await;

            // The state the datapoint implies, awaited the way a client polls for it: the
            // reader panics naming what the alarm last reported if it never arrives.
            let graded = await_state(&emulator, &case, METRIC_ALARMS, alarm, wanted).await;
            let reason = member(&graded, "StateReason");
            assert!(
                reason.starts_with(CROSSING_CLAUSE),
                "case {case:?}: expected an evaluated reason beginning {CROSSING_CLAUSE:?}, got \
                 {reason:?}"
            );

            // The transition is in the alarm's own history, as the one StateUpdate recording
            // the move out of the un-evaluated state. The item is SELECTED by the transition
            // it records rather than by being the only one: once the clock leaves the window
            // the row published into, the alarm legitimately returns to the un-evaluated state
            // and records that too, so only the count of items recording THIS transition is
            // time-independent.
            let updates = await_history(&emulator, alarm, "StateUpdate", 1).await;
            let transitions: Vec<&Value> = updates
                .iter()
                .filter(|item| {
                    recorded_previous_state(item) == UN_EVALUATED
                        && recorded_state(item) == wanted
                })
                .collect();
            assert_eq!(
                transitions.len(),
                1,
                "case {case:?}: expected exactly one item recording {UN_EVALUATED} -> {wanted}, \
                 got {updates:?}"
            );

            // … and the action it fired, as an Action item naming the topic it reached. Only
            // this one transition fires anything: the row configures no
            // `InsufficientDataActions`, so the return to the un-evaluated state publishes
            // nothing however long the row takes.
            let actions = await_history(&emulator, alarm, "Action", 1).await;
            assert_eq!(
                actions.len(),
                1,
                "case {case:?}: the one action of the transition must be recorded once, got \
                 {actions:?}"
            );
            let data = actions[0]["HistoryData"].as_str().unwrap_or_default();
            let recorded: Value = serde_json::from_str(data).unwrap_or_else(|error| {
                panic!("case {case:?}: HistoryData is not JSON ({error}): {data}")
            });
            assert_eq!(
                recorded["notificationResource"].as_str(),
                Some(topic.as_str()),
                "case {case:?}: the Action item must name the target it reached, got {recorded}"
            );
            assert_eq!(
                recorded["actionState"].as_str(),
                Some("Succeeded"),
                "case {case:?}: an action the target accepted is Succeeded, got {recorded}"
            );

            // And the notification is where a client waits for it: on the subscribed queue.
            let mut mailbox = Mailbox::over(&queue_url);
            let arrived = mailbox.await_for(&emulator, alarm, 1).await;
            assert_eq!(
                arrived.len(),
                1,
                "case {case:?}: the transition must deliver exactly one notification, got \
                 {arrived:?}"
            );
            let envelopes = mailbox.envelopes_of(alarm);
            let envelope = envelopes
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: the delivery was handed over with no \
                                           envelope: {envelopes:?}"));
            assert_eq!(
                envelope["TopicArn"].as_str(),
                Some(topic.as_str()),
                "case {case:?}: the delivery must name the topic it came from, got {envelope:?}"
            );
            let notification = arrived
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: no notification arrived: {arrived:?}"));
            assert_eq!(
                (
                    notification["OldStateValue"].as_str(),
                    notification["NewStateValue"].as_str()
                ),
                (Some(UN_EVALUATED), Some(wanted)),
                "case {case:?}: expected the notification to report {UN_EVALUATED} -> {wanted}, \
                 got {notification:?}"
            );
        }
    }
}
