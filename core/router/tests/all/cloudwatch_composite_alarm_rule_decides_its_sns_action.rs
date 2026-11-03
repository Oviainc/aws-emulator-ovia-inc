// SPDX-License-Identifier: CC0-1.0

//! A COMPOSITE alarm's rule decides whether its SNS action fires at all — through the WHOLE
//! mounted service set, the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **a composite alarm's `StateValue` is "the rule that this alarm uses to
//! evaluate its alarm state" (`CompositeAlarm$AlarmRule`) applied to the CURRENT states of the
//! alarms that rule references — so while the rule is NOT satisfied by those states the alarm
//! settles in `OK`, executes only the action list of that state and publishes nothing to the
//! topics its `AlarmActions` names, and once the rule IS satisfied it moves to `ALARM` and that
//! transition executes `AlarmActions` exactly once, publishing one notification that names the
//! alarm, the state it moved to, the state it moved from and the rule that was evaluated to the
//! queue subscribed to the topic.**
//!
//! Both halves of that rule are needed for either to mean anything: a queue that stays empty is
//! only evidence about the rule once the same wiring has been shown to DELIVER, and a delivery
//! is only evidence about the rule once the same wiring has been shown to stay quiet while the
//! rule is false. So every row establishes the positive control first — the composite's own
//! first evaluation computes `OK` and executes its `OKActions` on the same topic and the same
//! queue — then watches that queue stay empty while the rule is unsatisfied, and only then makes
//! the rule satisfied and reads the `ALARM` notification off it.
//!
//! Three services have to agree for that to hold — CloudWatch evaluates the rule and publishes,
//! SNS fans out, SQS holds what arrived — and the publish CloudWatch makes is an in-process call
//! addressed the way a client's own `sns`-scoped call is addressed, so WHICH mounted model claims
//! it is only decidable with every service mounted. The CloudWatch crate's own suites
//! (`services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/tests/all/`) mount
//! the three services they need and pin the rule's evaluation and its dispatch over tables of
//! rules, children and grants; the sibling router suites drive a METRIC alarm's action
//! (`cloudwatch_alarm_action_reaches_its_subscribed_queue.rs`, whose wire helpers this suite
//! reuses, and `cloudwatch_breaching_alarm_fires_through_the_whole_router.rs`), so no suite here
//! reached a COMPOSITE alarm's rule over the production service set.
//!
//! The rows differ along what the rule generalises over: which operators combine the state
//! functions and how they are grouped (a conjunction, a disjunction with an
//! `INSUFFICIENT_DATA` function, a negation, a negation inside a parenthesised conjunction),
//! which state function each child is read through, whether the rule references a child by name
//! or by ARN, how the subscribed queue's policy scopes the grant, and the Region the whole
//! wiring lives in.
//!
//! The children are two metric alarms over series NOTHING publishes a datapoint for, whose
//! states are therefore decided by their own missing-data treatment alone ("breaching" and
//! "notBreaching", `PutMetricAlarmInput$TreatMissingData`) and stand for as long as the row
//! runs: a row's rule is read over children that cannot drift under it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire walk, the topic-with-subscribed-queue wiring and the queue drain come from the
// sibling suite that owns them rather than a second copy of each.
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    cw, drain, drain_until, given_topic_with_subscriber, router, Grant,
};

/// The namespace and metric the children of every row watch, and which nothing publishes to.
const NAMESPACE: &str = "Custom/CompositeRuleWiring";
const METRIC: &str = "Depth";

/// The child whose missing-data treatment puts it in `ALARM`, and the one whose treatment puts
/// it in `OK`. Each row appends its own index to these stems, so a row's children, their series
/// and its composite alarm are its own even though every row is driven through ONE emulator.
const BREACHING_CHILD: &str = "rule-child-breaching";
const CLEAR_CHILD: &str = "rule-child-clear";

/// The composite alarm every row writes, under a name of its own.
const COMPOSITE: &str = "rule-parent";

/// The modelled alarm states this suite names.
const ALARM: &str = "ALARM";
const OK: &str = "OK";

/// How long a row waits for a state the stored alarms imply, or for the delivery that follows
/// one. Generous only so a failure is a failure rather than a hang: each transition is owed a
/// span inside the framework's window for a transition the clock makes due, and the delivery
/// follows it.
const DEADLINE: Duration = Duration::from_secs(30);

/// How often such a wait looks again.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// How long a row watches the queue for a delivery that must NOT be made: several evaluation
/// spans, so an action executed on a later evaluation of the same (unchanged) state, or on an
/// evaluation of the unsatisfied rule, would show up here.
const SETTLED_WATCH: Duration = Duration::from_secs(2);

/// One child: a static-threshold alarm over a series nothing publishes, so `treat_missing` alone
/// decides the state it settles in.
fn child_alarm(name: &str, treat_missing: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": NAMESPACE,
        "MetricName": METRIC,
        "Dimensions": [{"Name": "Child", "Value": name}],
        "Statistic": "Maximum",
        "Period": 60,
        "EvaluationPeriods": 1,
        "DatapointsToAlarm": 1,
        "Threshold": 50.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "TreatMissingData": treat_missing,
    })
}

/// The composite alarm of this row: both the `ALARM` and the `OK` list name the same topic, so
/// WHICH list a transition executed is read off the notification's own `NewStateValue` rather
/// than off which queue it landed in.
fn composite_alarm(name: &str, rule: &str, topic: &str) -> Value {
    json!({
        "AlarmName": name,
        "AlarmRule": rule,
        "AlarmActions": [topic],
        "OKActions": [topic],
    })
}

/// What `DescribeAlarms` reports for `name` once it reports `wanted` for it, under the
/// `collection` its alarm type is reported in.
///
/// Both alarm types are asked for: "If you omit this parameter, only metric alarms are
/// returned, even if composite alarms or log alarms exist in the account."
/// (`DescribeAlarmsInput$AlarmTypes`)
async fn awaited_state(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    collection: &str,
    name: &str,
    wanted: &str,
) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = cw(
            router,
            case,
            region,
            "DescribeAlarms",
            &json!({
                "AlarmNames": [name],
                "AlarmTypes": ["MetricAlarm", "CompositeAlarm"],
            }),
        )
        .await;
        let reported = described[collection][0].clone();
        if reported["StateValue"].as_str() == Some(wanted) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {collection} {name:?} to reach {wanted:?}, got {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every notification that reaches the queue at `url` until one of them carries `wanted` as the
/// state moved to, or the deadline passes.
async fn awaited_notifications(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    wanted: &str,
) -> Vec<Value> {
    let deadline = Instant::now() + DEADLINE;
    let mut arrived: Vec<Value> = Vec::new();
    loop {
        arrived.extend(drain(router, case, region, url).await);
        if arrived
            .iter()
            .any(|notification| notification["NewStateValue"].as_str() == Some(wanted))
        {
            return arrived;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a notification carrying {wanted:?}, got {arrived:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row: the same two children, read through a rule shape of its own.
struct Row {
    /// What this row is, as every assertion names it.
    case: &'static str,
    /// The Region the whole wiring lives in.
    region: &'static str,
    /// How the subscribed queue's policy grants SNS the send.
    grant: Grant,
    /// Whether the rule references its children by ARN rather than by name.
    by_arn: bool,
    /// The rule the children's states do NOT satisfy, over (the breaching child, the clear one).
    unsatisfied: fn(&str, &str) -> String,
    /// The rule those same states DO satisfy.
    satisfied: fn(&str, &str) -> String,
}

#[tokio::test]
async fn a_composite_alarms_rule_decides_whether_its_sns_action_fires() {
    let rows: &[Row] = &[
        Row {
            case: "a conjunction only one child satisfies",
            region: "us-west-2",
            grant: Grant::ScopedToTopic,
            by_arn: false,
            unsatisfied: |breaching, clear| {
                format!("ALARM(\"{breaching}\") AND ALARM(\"{clear}\")")
            },
            satisfied: |breaching, clear| format!("ALARM(\"{breaching}\") AND OK(\"{clear}\")"),
        },
        Row {
            case: "a disjunction neither child satisfies, referenced by ARN",
            region: "eu-west-1",
            grant: Grant::Unscoped,
            by_arn: true,
            unsatisfied: |breaching, clear| {
                format!("ALARM(\"{clear}\") OR INSUFFICIENT_DATA(\"{breaching}\")")
            },
            satisfied: |breaching, clear| format!("ALARM(\"{clear}\") OR ALARM(\"{breaching}\")"),
        },
        Row {
            case: "a negation the child's ALARM makes false",
            region: "ap-southeast-2",
            grant: Grant::ScopedToTopic,
            by_arn: false,
            unsatisfied: |breaching, _| format!("NOT ALARM(\"{breaching}\")"),
            satisfied: |_, clear| format!("NOT ALARM(\"{clear}\")"),
        },
        Row {
            case: "a negation inside a parenthesised conjunction",
            region: "us-east-1",
            grant: Grant::Unscoped,
            by_arn: false,
            unsatisfied: |breaching, clear| {
                format!("(ALARM(\"{breaching}\") OR ALARM(\"{clear}\")) AND NOT ALARM(\"{breaching}\")")
            },
            satisfied: |breaching, clear| {
                format!("(ALARM(\"{breaching}\") OR ALARM(\"{clear}\")) AND NOT ALARM(\"{clear}\")")
            },
        },
    ];

    // ONE emulator for every row, assembled the way the binary assembles it: each row names its
    // own topic, queue, children and composite alarm, so what a row's queue holds can only be
    // that row's, and the whole service set is registered once rather than four times.
    let router = router();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let composite = format!("{COMPOSITE}-{index}");
        let (topic, url) = given_topic_with_subscriber(
            &router,
            case,
            region,
            &format!("composite-actions-{index}"),
            row.grant,
        )
        .await;

        // The children, in the states their missing-data treatments decide, and the form this
        // row's rule references them in.
        let mut references: Vec<String> = Vec::new();
        for (stem, treat_missing, settles_in) in [
            (BREACHING_CHILD, "breaching", ALARM),
            (CLEAR_CHILD, "notBreaching", OK),
        ] {
            let child = format!("{stem}-{index}");
            cw(
                &router,
                case,
                region,
                "PutMetricAlarm",
                &child_alarm(&child, treat_missing),
            )
            .await;
            let settled =
                awaited_state(&router, case, region, "MetricAlarms", &child, settles_in).await;
            references.push(if row.by_arn {
                settled["AlarmArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no AlarmArn in {settled}"))
                    .to_string()
            } else {
                child
            });
        }

        // The composite over the rule those states do not satisfy.
        let unsatisfied = (row.unsatisfied)(&references[0], &references[1]);
        cw(
            &router,
            case,
            region,
            "PutCompositeAlarm",
            &composite_alarm(&composite, &unsatisfied, &topic),
        )
        .await;

        // The positive control: the first evaluation computes the rule's value — false, so OK —
        // and executes the action list of THAT state, which the subscribed queue receives. The
        // same topic, subscription and grant carry what the next leg must not see.
        let reported =
            awaited_state(&router, case, region, "CompositeAlarms", &composite, OK).await;
        assert_eq!(
            reported["AlarmRule"].as_str(),
            Some(unsatisfied.as_str()),
            "case {case:?}: the alarm must report the rule it was written with, got {reported:?}"
        );
        let control = drain_until(&router, case, region, &url, 1).await;
        assert_eq!(
            control.len(),
            1,
            "case {case:?}: the first evaluation must deliver exactly one notification, got \
             {control:?}"
        );
        assert_eq!(
            control[0]["AlarmName"].as_str(),
            Some(composite.as_str()),
            "case {case:?}: the notification must name the composite alarm, got {:?}",
            control[0]
        );
        assert_eq!(
            control[0]["NewStateValue"].as_str(),
            Some(OK),
            "case {case:?}: an unsatisfied rule computes OK, so the OK list is what the first \
             evaluation executes, got {:?}",
            control[0]
        );

        // The rule stays unsatisfied, so the alarm stays out of ALARM and nothing more is
        // published — the no-action outcome, read on a queue that has just been shown to
        // deliver.
        tokio::time::sleep(SETTLED_WATCH).await;
        let quiet = drain(&router, case, region, &url).await;
        assert!(
            quiet.is_empty(),
            "case {case:?}: an unsatisfied rule executes nothing further, so the queue must stay \
             empty, got {quiet:?}"
        );
        let stayed = cw(
            &router,
            case,
            region,
            "DescribeAlarms",
            &json!({"AlarmNames": [&composite], "AlarmTypes": ["CompositeAlarm"]}),
        )
        .await;
        assert_eq!(
            stayed["CompositeAlarms"][0]["StateValue"].as_str(),
            Some(OK),
            "case {case:?}: an unsatisfied rule keeps the alarm out of ALARM, got {stayed}"
        );

        // And the quiet is the RULE's doing: over the very same children, states and wiring, a
        // rule they do satisfy moves the alarm to ALARM and executes the ALARM list once.
        let satisfied = (row.satisfied)(&references[0], &references[1]);
        cw(
            &router,
            case,
            region,
            "PutCompositeAlarm",
            &composite_alarm(&composite, &satisfied, &topic),
        )
        .await;
        awaited_state(&router, case, region, "CompositeAlarms", &composite, ALARM).await;
        let delivered = awaited_notifications(&router, case, region, &url, ALARM).await;
        // The control notification was taken off the queue above and the alarm held OK until
        // this rewrite, so the ONE transition it made is the only thing the queue can hold: the
        // whole set is asserted rather than the ALARM entries filtered out of it.
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the transition into ALARM must execute the ALARM list exactly once \
             and nothing else, got {delivered:?}"
        );
        let fired = &delivered[0];
        assert_eq!(
            fired["NewStateValue"].as_str(),
            Some(ALARM),
            "case {case:?}: a satisfied rule computes ALARM, so the ALARM list is what the \
             evaluation executes, got {fired:?}"
        );
        assert_eq!(
            fired["OldStateValue"].as_str(),
            Some(OK),
            "case {case:?}: the notification must carry the state the alarm moved from, got \
             {fired:?}"
        );
        assert_eq!(
            fired["AlarmRule"].as_str(),
            Some(satisfied.as_str()),
            "case {case:?}: the notification must report the rule that was evaluated, got \
             {fired:?}"
        );
    }
}
