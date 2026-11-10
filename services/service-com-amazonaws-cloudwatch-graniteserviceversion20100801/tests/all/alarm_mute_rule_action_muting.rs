// SPDX-License-Identifier: CC0-1.0

//! What an ACTIVE alarm mute rule does to the actions an alarm's transition fires, end to end
//! over the wire: "When a mute rule is active, the targeted alarms continue to evaluate metrics
//! and transition between states, but their configured actions (such as Amazon SNS notifications
//! or Auto Scaling actions) are muted." (`PutAlarmMuteRule`).
//!
//! So one rule is asserted here, over a TABLE whose rows differ along everything that rule
//! generalises over — whether a mute window holds the instant of the transition (open now, still
//! to come, already closed, past the date the rule is no longer evaluated), which expression form
//! states that window, whether the rule names this alarm among its targets, which Region the rule
//! was written in, how many rules the store holds, which kind of alarm transitions and which state
//! it moves to. For every row: the alarm transitions either way, a muted transition delivers
//! NOTHING to the target its actions name and records in the alarm's own history that a named mute
//! rule muted them, and an unmuted one delivers the notification and records no mute at all.

use serde_json::{json, Value};

use crate::harness::{
    element, emulator, given_queue, grant_sns_delivery_on, received_messages, rpc, rpc_in_region,
    set_alarm_state, sns, subscribe_queue, Emulator, REGION,
};

/// The CloudWatch signing name and target prefix, for `rpc`.
const CLOUDWATCH: (&str, &str) = ("monitoring", "GraniteServiceVersion20100801");

/// Another Region, for the row that asserts a mute rule only mutes the alarms of its own Region.
const OTHER_REGION: &str = "us-west-2";

/// How many times a row reads for something it expects to arrive, 100 ms apart: a delivery is
/// asynchronous, so a row waits for the notification instead of assuming the call that fired it
/// also delivered it.
const READS: usize = 40;

/// How many of those reads a row makes before concluding that NOTHING was delivered: the same
/// path, given time to deliver, so an empty queue is an answer rather than a race.
const QUIET_READS: usize = 6;

/// Call one CloudWatch operation in the Region the alarms live in, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, CLOUDWATCH.0, CLOUDWATCH.1, operation, body).await
}

/// Which alarms a row's mute rule names as its targets (`MuteTargets$AlarmNames`).
#[derive(Debug, Clone, Copy)]
enum Targets {
    /// The alarm the row drives.
    ThisAlarm,
    /// An alarm of another name, which this one is not.
    AnotherAlarm,
    /// No `MuteTargets` member at all, so the rule names no alarm.
    Unset,
}

/// One mute rule a row writes.
#[derive(Debug, Clone)]
struct RuleSpec {
    /// The schedule expression, in one of the two documented forms (`Schedule$Expression`).
    expression: String,
    /// How long a window of it stays open (`Schedule$Duration`).
    duration: &'static str,
    /// Which alarms it applies to.
    targets: Targets,
    /// The instant after which it "is no longer evaluated"
    /// (`PutAlarmMuteRuleInput$ExpireDate`), when the row sets one.
    expire_date: Option<i64>,
    /// The Region the rule is written in, which is the Region it lives in.
    region: &'static str,
}

/// One row: the alarm it drives, the rules the store holds while it transitions, the state it
/// forces, and whether that transition's actions must be muted.
struct Row {
    /// What the row is, named in every assertion it makes.
    case: &'static str,
    /// Whether the alarm that transitions is a COMPOSITE alarm rather than a metric alarm.
    composite: bool,
    /// The state the row forces the alarm into, which selects the actions the transition fires.
    forced_state: &'static str,
    /// The mute rules written before the transition.
    rules: Vec<RuleSpec>,
    /// Which of those rules must mute the transition's actions, as its index in `rules`, or
    /// `None` when the transition's actions must be executed.
    muting_rule: Option<usize>,
}

/// An `at` expression naming `at` as the local wall-clock instant UTC puts it at: "At expressions
/// … Format: `at(yyyy-MM-ddThh:mm)`" (`Schedule$Expression`), read in UTC because no row names a
/// time zone ("If you don't specify a time zone, UTC is used by default.", `Schedule$Timezone`).
fn at_expression(at: ::chrono::DateTime<::chrono::Utc>) -> String {
    format!("at({})", at.format("%Y-%m-%dT%H:%M"))
}

/// The `PutAlarmMuteRule` request for `spec`, named `rule_name` and resolved against the row's
/// own `alarm_name`.
fn rule_request(rule_name: &str, spec: &RuleSpec, alarm_name: &str) -> Value {
    let mut request = json!({
        "Name": rule_name,
        "Rule": {"Schedule": {"Expression": spec.expression, "Duration": spec.duration}},
    });
    let members = request
        .as_object_mut()
        .expect("the request being built is a JSON object");
    match spec.targets {
        Targets::ThisAlarm => {
            members.insert("MuteTargets".to_string(), json!({"AlarmNames": [alarm_name]}));
        }
        Targets::AnotherAlarm => {
            members.insert(
                "MuteTargets".to_string(),
                json!({"AlarmNames": [format!("{alarm_name}-untargeted")]}),
            );
        }
        Targets::Unset => {}
    }
    if let Some(expire_date) = spec.expire_date {
        members.insert("ExpireDate".to_string(), json!(expire_date));
    }
    request
}

/// Create the alarm this row drives — a metric alarm, or a composite alarm over a child metric
/// alarm — with `topic_arn` on the action lists of both states a row forces.
async fn given_alarm(emulator: &Emulator, row: &Row, name: &str, topic_arn: &str) {
    if row.composite {
        let child = format!("{name}-child");
        cw(
            emulator,
            "PutMetricAlarm",
            &json!({
                "AlarmName": child,
                "Namespace": "Custom/Muting",
                "MetricName": "Latency",
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 10.0,
                "ComparisonOperator": "GreaterThanThreshold",
            }),
        )
        .await;
        cw(
            emulator,
            "PutCompositeAlarm",
            &json!({
                "AlarmName": name,
                "AlarmRule": format!("ALARM(\"{child}\")"),
                "AlarmActions": [topic_arn],
                "OKActions": [topic_arn],
            }),
        )
        .await;
        return;
    }
    cw(
        emulator,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "Custom/Muting",
            "MetricName": "Latency",
            "Dimensions": [{"Name": "Alarm", "Value": name}],
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 10.0,
            "ComparisonOperator": "GreaterThanThreshold",
            "AlarmActions": [topic_arn],
            "OKActions": [topic_arn],
        }),
    )
    .await;
}

/// The alarm history of `name`, for an alarm of `alarm_type`, whatever item type each item is.
async fn history(emulator: &Emulator, name: &str, alarm_type: &str) -> Vec<Value> {
    let reported = cw(
        emulator,
        "DescribeAlarmHistory",
        &json!({"AlarmName": name, "AlarmTypes": [alarm_type]}),
    )
    .await;
    reported["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every message the queue at `url` holds, read until it holds at least `want` of them.
async fn delivered(emulator: &Emulator, url: &str, want: usize, reads: usize) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    for _ in 0..reads {
        messages.extend(received_messages(emulator, url).await);
        if messages.len() >= want {
            break;
        }
        ::tokio::time::sleep(::std::time::Duration::from_millis(100)).await;
    }
    messages
}

/// Read the history of `name` until an item mentions `mute_rule`, and answer every item it holds.
async fn history_mentioning(
    emulator: &Emulator,
    name: &str,
    alarm_type: &str,
    mute_rule: &str,
) -> Vec<Value> {
    let mut items: Vec<Value> = Vec::new();
    for _ in 0..READS {
        items = history(emulator, name, alarm_type).await;
        if items.iter().any(|item| mentions(item, mute_rule)) {
            break;
        }
        ::tokio::time::sleep(::std::time::Duration::from_millis(100)).await;
    }
    items
}

/// Whether a history item names `mute_rule` anywhere a reader of the history looks: its summary
/// ("A summary of the alarm history, in text format.", `AlarmHistoryItem$HistorySummary`) or its
/// data ("Data about the alarm, in JSON format.", `AlarmHistoryItem$HistoryData`).
fn mentions(item: &Value, mute_rule: &str) -> bool {
    ["HistorySummary", "HistoryData"]
        .iter()
        .filter_map(|member| item.get(*member).and_then(Value::as_str))
        .any(|text| text.contains(mute_rule))
}

/// The state value the alarm `name` of `alarm_type` reports.
async fn reported_state(emulator: &Emulator, name: &str, alarm_type: &str) -> Option<String> {
    let reported = cw(
        emulator,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": [alarm_type]}),
    )
    .await;
    let list = if alarm_type == "CompositeAlarm" {
        "CompositeAlarms"
    } else {
        "MetricAlarms"
    };
    reported[list][0]
        .get("StateValue")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[tokio::test]
async fn an_active_mute_rule_mutes_the_actions_a_targeted_alarm_s_transition_fires() {
    let emulator = emulator();
    let now = ::chrono::Utc::now();
    let open_now = at_expression(now - ::chrono::TimeDelta::minutes(1));
    let still_to_come = at_expression(now + ::chrono::TimeDelta::hours(2));
    let already_closed = at_expression(now - ::chrono::TimeDelta::hours(2));

    /// A rule of the row's own Region targeting the row's own alarm, with the window `expression`
    /// opens for `duration`.
    fn targeting(expression: &str, duration: &'static str) -> RuleSpec {
        RuleSpec {
            expression: expression.to_string(),
            duration,
            targets: Targets::ThisAlarm,
            expire_date: None,
            region: REGION,
        }
    }

    let rows: Vec<Row> = vec![
        Row {
            case: "a one-time window open now, targeting the alarm",
            composite: false,
            forced_state: "ALARM",
            rules: vec![targeting(&open_now, "PT1H")],
            muting_rule: Some(0),
        },
        Row {
            case: "a recurring window that opens every minute, targeting the alarm",
            composite: false,
            forced_state: "ALARM",
            rules: vec![targeting("cron(* * * * *)", "PT1H")],
            muting_rule: Some(0),
        },
        Row {
            case: "the shortest window a schedule may state, open now",
            composite: false,
            forced_state: "ALARM",
            rules: vec![targeting("cron(* * * * *)", "PT1M")],
            muting_rule: Some(0),
        },
        Row {
            case: "the OK transition of a targeted alarm",
            composite: false,
            forced_state: "OK",
            rules: vec![targeting(&open_now, "PT1H")],
            muting_rule: Some(0),
        },
        Row {
            case: "a composite alarm targeted by an open window",
            composite: true,
            forced_state: "ALARM",
            rules: vec![targeting(&open_now, "PT1H")],
            muting_rule: Some(0),
        },
        Row {
            case: "three rules, only one of them open now",
            composite: false,
            forced_state: "ALARM",
            rules: vec![
                targeting(&still_to_come, "PT1H"),
                targeting(&open_now, "PT4H"),
                RuleSpec {
                    targets: Targets::AnotherAlarm,
                    ..targeting("cron(* * * * *)", "PT1H")
                },
            ],
            muting_rule: Some(1),
        },
        Row {
            case: "a window still to come",
            composite: false,
            forced_state: "ALARM",
            rules: vec![targeting(&still_to_come, "PT1H")],
            muting_rule: None,
        },
        Row {
            case: "a one-time window that has already closed",
            composite: false,
            forced_state: "ALARM",
            rules: vec![targeting(&already_closed, "PT1H")],
            muting_rule: None,
        },
        Row {
            case: "a recurring window past the date the rule is no longer evaluated",
            composite: false,
            forced_state: "ALARM",
            rules: vec![RuleSpec {
                expire_date: Some((now - ::chrono::TimeDelta::minutes(1)).timestamp()),
                ..targeting("cron(* * * * *)", "PT1H")
            }],
            muting_rule: None,
        },
        Row {
            case: "an open window targeting another alarm",
            composite: false,
            forced_state: "ALARM",
            rules: vec![RuleSpec {
                targets: Targets::AnotherAlarm,
                ..targeting(&open_now, "PT1H")
            }],
            muting_rule: None,
        },
        Row {
            case: "an open window naming no targets at all",
            composite: false,
            forced_state: "ALARM",
            rules: vec![RuleSpec {
                targets: Targets::Unset,
                ..targeting(&open_now, "PT1H")
            }],
            muting_rule: None,
        },
        Row {
            case: "an open window targeting the alarm, written in another Region",
            composite: false,
            forced_state: "ALARM",
            rules: vec![RuleSpec {
                region: OTHER_REGION,
                ..targeting(&open_now, "PT1H")
            }],
            muting_rule: None,
        },
        Row {
            case: "a composite alarm no rule targets",
            composite: true,
            forced_state: "ALARM",
            rules: vec![RuleSpec {
                targets: Targets::AnotherAlarm,
                ..targeting(&open_now, "PT1H")
            }],
            muting_rule: None,
        },
        Row {
            case: "no mute rule at all",
            composite: false,
            forced_state: "ALARM",
            rules: Vec::new(),
            muting_rule: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let alarm_name = format!("muting-{index}");
        let alarm_type = if row.composite {
            "CompositeAlarm"
        } else {
            "MetricAlarm"
        };

        // The path a notification travels: a queue subscribed to the topic the alarm's actions
        // name, which is where a client sees the notification — or sees that none arrived.
        let (queue_url, queue_arn) = given_queue(&emulator, &alarm_name).await;
        grant_sns_delivery_on(&emulator, &queue_url, &queue_arn).await;
        let topic_arn = element(
            &sns(&emulator, "CreateTopic", &[("Name", &alarm_name)]).await,
            "TopicArn",
        );
        subscribe_queue(&emulator, &topic_arn, &queue_arn).await;
        given_alarm(&emulator, row, &alarm_name, &topic_arn).await;

        let mut rule_names: Vec<String> = Vec::new();
        for (ordinal, spec) in row.rules.iter().enumerate() {
            let rule_name = format!("{alarm_name}-rule-{ordinal}");
            let request = rule_request(&rule_name, spec, &alarm_name);
            rpc_in_region(
                &emulator,
                spec.region,
                CLOUDWATCH.0,
                CLOUDWATCH.1,
                "PutAlarmMuteRule",
                &request,
            )
            .await;
            rule_names.push(rule_name);
        }

        // The transition itself: the state the alarm moves to is what selects the actions it
        // fires, muted or not.
        set_alarm_state(&emulator, &alarm_name, row.forced_state).await;

        // "the targeted alarms continue to evaluate metrics and transition between states"
        // (`MuteTargets`) — a mute changes the ACTIONS, never the transition.
        let state = reported_state(&emulator, &alarm_name, alarm_type).await;
        assert_eq!(
            state.as_deref(),
            Some(row.forced_state),
            "case {case:?}: the alarm must report the state it moved to, got {state:?}"
        );

        if let Some(muting) = row.muting_rule {
            let quiet = delivered(&emulator, &queue_url, 1, QUIET_READS).await;
            assert!(
                quiet.is_empty(),
                "case {case:?}: a muted transition must deliver nothing, got {quiet:?}"
            );
            let muting = rule_names[muting].clone();
            let items = history_mentioning(&emulator, &alarm_name, alarm_type, &muting).await;
            assert!(
                items.iter().any(|item| mentions(item, &muting)),
                "case {case:?}: the history must record that mute rule {muting:?} muted the \
                 actions, got {items:?}"
            );
            let executed: Vec<&Value> = items
                .iter()
                .filter(|item| {
                    item.get("HistorySummary")
                        .and_then(Value::as_str)
                        .is_some_and(|summary| summary.contains("executed action"))
                })
                .collect();
            assert!(
                executed.is_empty(),
                "case {case:?}: a muted transition must record no action execution, got \
                 {executed:?}"
            );
        } else {
            let notified = delivered(&emulator, &queue_url, 1, READS).await;
            assert_eq!(
                notified.len(),
                1,
                "case {case:?}: an unmuted transition must deliver its notification, got \
                 {notified:?}"
            );
            let items = history(&emulator, &alarm_name, alarm_type).await;
            let muted: Vec<&Value> = items
                .iter()
                .filter(|item| rule_names.iter().any(|rule| mentions(item, rule)))
                .collect();
            assert!(
                muted.is_empty(),
                "case {case:?}: no mute rule may be recorded against an unmuted transition, got \
                 {muted:?}"
            );
        }
    }
}
