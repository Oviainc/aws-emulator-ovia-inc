// SPDX-License-Identifier: CC0-1.0

//! `PutScheduledAction` and `DescribeScheduledActions`, end to end.
//!
//! The rules asserted here, each over a table of inputs:
//!
//! * a schedule is one of the three documented expressions — "at(yyyy-mm-ddThh:mm:ss)",
//!   "rate(value unit)" or "cron(fields)", the cron form being "six fields separated by
//!   white spaces: [Minutes] [Hours] [Day_of_Month] [Month] [Day_of_Week] [Year]" — and
//!   anything else is refused with 400 `ValidationException`, because a schedule that names
//!   no instant can never run;
//! * `ScheduledActionName` carries the model's own pattern
//!   `^(?!((^[ ]+.*)|(.*([\u0000-\u001f]|[\u007f-\u009f]|[:/|])+.*)|(.*[ ]+$))).+$`, so a
//!   name with a `/`, a `:`, a `|`, a control character or a leading/trailing space is
//!   refused the same way;
//! * an action exists against a REGISTERED target, round-trips through
//!   `DescribeScheduledActions`, and — "at the scheduled time, if the current capacity is
//!   below the minimum capacity, Application Auto Scaling scales out to the minimum
//!   capacity" — moves the target's capacity bounds once its occurrence has come due.

use serde_json::{json, Value};

use super::emulator::{at_schedule_in, error_code, error_message, Emulator};

/// The scalable target every case below schedules against.
const NAMESPACE: &str = "dynamodb";
const RESOURCE_ID: &str = "table/orders";
const DIMENSION: &str = "dynamodb:table:ReadCapacityUnits";

/// An emulator with the table created and the target registered.
async fn registered() -> Emulator {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("orders", 1, 1).await;
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({"ServiceNamespace": NAMESPACE, "ResourceId": RESOURCE_ID,
                   "ScalableDimension": DIMENSION, "MinCapacity": 1, "MaxCapacity": 20}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"register\": expected 200, got {status} {body:?}"
    );
    emulator
}

/// A `PutScheduledAction` request for `name` and `schedule`.
fn action(name: &str, schedule: &str) -> Value {
    json!({
        "ServiceNamespace": NAMESPACE,
        "ResourceId": RESOURCE_ID,
        "ScalableDimension": DIMENSION,
        "ScheduledActionName": name,
        "Schedule": schedule,
    })
}

/// The schedule expressions the service documents, and forms that are not schedules.
///
/// The accepted rows walk all three documented forms and the cron features the six-field
/// grammar allows (a `?` for "no specific value", a step, a list, a named month); the
/// refused rows are the same forms broken in one way each.
const SCHEDULES: &[(&str, &str, bool)] = &[
    ("a one-off at expression", "at(2030-01-15T13:30:00)", true),
    ("a rate in minutes", "rate(5 minutes)", true),
    ("a rate of one hour", "rate(1 hour)", true),
    ("a rate in days", "rate(2 days)", true),
    ("a cron with ? for the day of week", "cron(0 10 * * ? *)", true),
    ("a cron with a step", "cron(0/15 * * * ? *)", true),
    ("a cron with a list and a named month", "cron(0 8,20 1 JAN ? *)", true),
    ("a cron with a day-of-week range", "cron(30 9 ? * MON-FRI *)", true),
    ("an at expression that is not a date", "at(tomorrow)", false),
    ("an at expression with an impossible date", "at(2030-13-45T99:99:99)", false),
    ("a rate of zero", "rate(0 minutes)", false),
    ("a rate in an unknown unit", "rate(5 fortnights)", false),
    ("a rate with no unit", "rate(5)", false),
    ("a cron of five fields", "cron(0 10 * * ?)", false),
    ("a cron of seven fields", "cron(0 0 10 * * ? *)", false),
    ("a cron whose minute field is not a minute", "cron(sixty 10 * * ? *)", false),
    ("a cron whose hour is out of range", "cron(0 99 * * ? *)", false),
    ("an expression in no documented form", "every monday at 9", false),
    ("an empty expression", "cron()", false),
    // A magnitude no instant arithmetic can represent is refused at the boundary rather
    // than carried into the clock pass.
    ("a rate of u64::MAX days", "rate(18446744073709551615 days)", false),
    ("a rate past the hundred-year horizon", "rate(1000000 days)", false),
];

#[tokio::test]
async fn a_schedule_is_one_of_the_documented_expressions() {
    let emulator = registered().await;
    for (index, (case, schedule, accepted)) in SCHEDULES.iter().enumerate() {
        let name = format!("action-{index}");
        let (status, body) = emulator
            .call("PutScheduledAction", action(&name, schedule))
            .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({schedule:?}): expected the schedule to be accepted, got \
                 {status} {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?} ({schedule:?}): expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} ({schedule:?}): expected a ValidationException, got {body:?}"
        );
        assert!(
            error_message(&body).contains("schedule"),
            "case {case:?} ({schedule:?}): expected the message to name the member, got {body:?}"
        );
    }

    // Only the accepted rows were stored.
    let accepted = SCHEDULES.iter().filter(|(_, _, ok)| *ok).count();
    let (status, body) = emulator
        .call(
            "DescribeScheduledActions",
            json!({"ServiceNamespace": NAMESPACE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"describe\": expected 200, got {status} {body:?}"
    );
    let actions = body["ScheduledActions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        actions.len(),
        accepted,
        "case \"describe\": expected exactly the accepted schedules to be stored, got {actions:?}"
    );
}

/// Names the model's pattern allows, and names it forbids.
const NAMES: &[(&str, &str, bool)] = &[
    ("a plain name", "nightly-scale-out", true),
    ("a name with dots and underscores", "scale.out_2", true),
    ("a name with an inner space", "nightly scale out", true),
    ("a name with a slash", "nightly/scale-out", false),
    ("a name with a colon", "nightly:scale-out", false),
    ("a name with a pipe", "nightly|scale-out", false),
    ("a name with a leading space", " nightly", false),
    ("a name with a trailing space", "nightly ", false),
    ("a name with a control character", "nightly\u{1}", false),
    ("a name with a C1 control character", "nightly\u{85}", false),
];

#[tokio::test]
async fn a_scheduled_action_name_follows_the_modelled_pattern() {
    let emulator = registered().await;
    for (case, name, accepted) in NAMES {
        let (status, body) = emulator
            .call("PutScheduledAction", action(name, "rate(5 minutes)"))
            .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({name:?}): expected the name to be accepted, got {status} \
                 {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?} ({name:?}): expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} ({name:?}): expected a ValidationException, got {body:?}"
        );
    }
}

/// The time zones the model's `Timezone` member accepts, and names that are not zones.
///
/// "Valid values are the canonical names of the IANA time zones supported by Joda-Time
/// (such as Etc/GMT+9 or Pacific/Tahiti)" — resolved against the zone database itself, so
/// the accepted rows are canonical names the issue did not cite and the refused ones are
/// an abbreviation, a zone that does not exist and an empty name.
const TIMEZONES: &[(&str, &str, bool)] = &[
    ("the default zone, named", "UTC", true),
    ("the doc's first example", "Etc/GMT+9", true),
    ("the doc's second example", "Pacific/Tahiti", true),
    ("a zone with three segments", "America/Argentina/Ushuaia", true),
    ("an abbreviation, not a canonical name", "PST", false),
    ("a zone that does not exist", "Mars/Olympus", false),
    ("an area with no location", "America/", false),
    ("an empty name", " ", false),
];

#[tokio::test]
async fn a_timezone_is_a_canonical_iana_zone_name() {
    let emulator = registered().await;
    for (index, (case, timezone, accepted)) in TIMEZONES.iter().enumerate() {
        let mut request = action(&format!("zoned-{index}"), "cron(0 10 * * ? *)");
        request["Timezone"] = json!(timezone);
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?} ({timezone:?}): expected the zone to be accepted, got {status} \
                 {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?} ({timezone:?}): expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?} ({timezone:?}): expected a ValidationException, got {body:?}"
        );
        assert!(
            error_message(&body).contains("timezone"),
            "case {case:?} ({timezone:?}): expected the message to name the member, got {body:?}"
        );
    }
}

/// An instant outside the range the service computes in is refused, and the service keeps
/// answering: no request-derived value reaches arithmetic that cannot represent it.
#[tokio::test]
async fn an_instant_outside_the_representable_range_is_refused_and_wedges_nothing() {
    let emulator = registered().await;
    for (case, member, value) in [
        ("a StartTime past the year 9999", "StartTime", 1e30),
        ("an EndTime past the year 9999", "EndTime", 253_402_300_800_f64),
        ("a StartTime before the epoch", "StartTime", -1e12),
    ] {
        let mut request = action("bounded", "rate(5 minutes)");
        request[member] = json!(value);
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );

        // And the next request still answers: nothing was persisted that a later clock
        // pass cannot compute with.
        let (status, body) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": NAMESPACE}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the service to keep answering, got {status} {body:?}"
        );
    }

    // The largest instant the service does compute in is accepted, and every later read
    // keeps working with it stored.
    let mut request = action("at-the-horizon", "rate(5 minutes)");
    request["EndTime"] = json!(253_402_300_799_f64);
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"the horizon itself\": expected 200, got {status} {body:?}"
    );
    for operation in ["DescribeScheduledActions", "DescribeScalableTargets"] {
        let (status, body) = emulator
            .call(operation, json!({"ServiceNamespace": NAMESPACE}))
            .await;
        assert_eq!(
            status, 200,
            "case \"the horizon itself\" on {operation}: expected 200, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn a_scheduled_action_round_trips_and_is_filtered_by_its_target() {
    let emulator = registered().await;
    let mut request = action("nightly", "cron(0 10 * * ? *)");
    request["Timezone"] = json!("Pacific/Tahiti");
    request["StartTime"] = json!(1_900_000_000);
    request["EndTime"] = json!(1_900_086_400_i64);
    request["ScalableTargetAction"] = json!({"MinCapacity": 4, "MaxCapacity": 12});
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"put\": expected 200, got {status} {body:?}"
    );

    let (status, body) = emulator
        .call(
            "DescribeScheduledActions",
            json!({"ServiceNamespace": NAMESPACE, "ResourceId": RESOURCE_ID,
                   "ScalableDimension": DIMENSION, "ScheduledActionNames": ["nightly"]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"describe\": expected 200, got {status} {body:?}"
    );
    let actions = body["ScheduledActions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        actions.len(),
        1,
        "case \"describe\": expected exactly the action that was put, got {actions:?}"
    );
    let stored = &actions[0];
    for (member, expected) in [
        ("ScheduledActionName", json!("nightly")),
        ("ServiceNamespace", json!(NAMESPACE)),
        ("ResourceId", json!(RESOURCE_ID)),
        ("ScalableDimension", json!(DIMENSION)),
        ("Schedule", json!("cron(0 10 * * ? *)")),
        ("Timezone", json!("Pacific/Tahiti")),
        (
            "ScalableTargetAction",
            json!({"MinCapacity": 4, "MaxCapacity": 12}),
        ),
    ] {
        assert_eq!(
            stored[member], expected,
            "case {member:?}: expected the value that was put, got {stored:?}"
        );
    }
    let arn = stored["ScheduledActionARN"].as_str().unwrap_or_default();
    assert!(
        arn.contains(":scheduledActionName/nightly") && arn.contains(":resource/dynamodb/"),
        "case \"arn\": expected an ARN naming the resource and the action, got {arn:?}"
    );
    assert!(
        stored["CreationTime"].as_f64().is_some_and(|at| at > 0.0),
        "case \"creation time\": expected a CreationTime after the epoch, got {stored:?}"
    );

    // A filter that names another resource matches nothing.
    let (status, body) = emulator
        .call(
            "DescribeScheduledActions",
            json!({"ServiceNamespace": NAMESPACE, "ResourceId": "table/other"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"filter by another resource\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScheduledActions"],
        json!([]),
        "case \"filter by another resource\": expected no match, got {body:?}"
    );

    // A re-put of the same name updates it rather than adding a second action, and the
    // instants it leaves out are deleted: "if you don't specify start and end times, the
    // old values are deleted".
    let (status, body) = emulator
        .call(
            "PutScheduledAction",
            action("nightly", "rate(1 hour)"),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"re-put\": expected 200, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call(
            "DescribeScheduledActions",
            json!({"ServiceNamespace": NAMESPACE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"describe after re-put\": expected 200, got {status} {body:?}"
    );
    let actions = body["ScheduledActions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        actions.len(),
        1,
        "case \"describe after re-put\": expected one action, got {actions:?}"
    );
    assert_eq!(
        actions[0]["Schedule"],
        json!("rate(1 hour)"),
        "case \"describe after re-put\": expected the new schedule, got {actions:?}"
    );
    assert_eq!(
        actions[0]["ScheduledActionARN"],
        json!(arn),
        "case \"describe after re-put\": expected the ARN to survive the update, got {actions:?}"
    );
    assert!(
        actions[0].get("StartTime").is_none() && actions[0].get("EndTime"). is_none(),
        "case \"describe after re-put\": expected the old instants to be deleted, got {actions:?}"
    );
}

#[tokio::test]
async fn an_action_whose_time_has_come_moves_the_targets_capacity_bounds() {
    let emulator = registered().await;
    // An `at` expression a few seconds out: once that instant passes, the occurrence was
    // owed whether or not anybody called, so the next report shows the bounds it set.
    let (schedule, at) = at_schedule_in(std::time::Duration::from_secs(5));
    let mut request = action("due-shortly", &schedule);
    request["ScalableTargetAction"] = json!({"MinCapacity": 6, "MaxCapacity": 30});
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"put\": expected 200, got {status} {body:?}"
    );

    // The same wall-clock reading in a zone twelve hours behind UTC is a different
    // instant — still half a day away — so that action must not fire with this one.
    let zoned = registered().await;
    let mut request = action("due-shortly-elsewhere", &schedule);
    request["Timezone"] = json!("Etc/GMT+12");
    request["ScalableTargetAction"] = json!({"MinCapacity": 7});
    let (status, body) = zoned.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"put in another zone\": expected 200, got {status} {body:?}"
    );

    // Before the instant, nothing has moved. (The window is the five seconds above, so
    // this reads the state only while the occurrence is still ahead; the assertion is
    // skipped if the machine was slow enough to cross it, which the case below covers
    // deterministically.)
    if std::time::SystemTime::now() < at {
        let (status, body) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": NAMESPACE}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"before the occurrence\": expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["ScalableTargets"][0]["MinCapacity"],
            json!(1),
            "case \"before the occurrence\": expected the registered bounds, got {body:?}"
        );
    }

    // Nothing sleeps in the service: the due instant is stored and materialised by the
    // next request after it passes.
    let remaining = at
        .duration_since(std::time::SystemTime::now())
        .unwrap_or_default();
    std::thread::sleep(remaining + std::time::Duration::from_millis(1_500));

    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": NAMESPACE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"describe\": expected 200, got {status} {body:?}"
    );
    let target = &body["ScalableTargets"][0];
    assert_eq!(
        target["MinCapacity"],
        json!(6),
        "case \"describe\": expected the action's MinCapacity to be in effect, got {target:?}"
    );
    assert_eq!(
        target["MaxCapacity"],
        json!(30),
        "case \"describe\": expected the action's MaxCapacity to be in effect, got {target:?}"
    );

    // The movement the occurrence made is a scaling activity, like every other capacity
    // movement this service makes, and it names the action that caused it.
    let (status, body) = emulator
        .call(
            "DescribeScalingActivities",
            json!({"ServiceNamespace": NAMESPACE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"activities\": expected 200, got {status} {body:?}"
    );
    let activities = body["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        activities.iter().any(|activity| {
            activity["Cause"]
                .as_str()
                .unwrap_or_default()
                .contains("due-shortly")
                && activity["Description"]
                    .as_str()
                    .unwrap_or_default()
                    .contains('6')
        }),
        "case \"activities\": expected an activity for the scheduled movement to 6, got \
         {activities:?}"
    );

    // The zoned action's instant has not arrived.
    let (status, body) = zoned
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": NAMESPACE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"another zone\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalableTargets"][0]["MinCapacity"],
        json!(1),
        "case \"another zone\": expected the same wall-clock reading twelve hours behind UTC \
         not to have fired yet, got {body:?}"
    );

    // An action whose occurrence is still ahead has not moved anything.
    let emulator = registered().await;
    let mut request = action("not-yet", "at(2037-01-01T00:00:00)");
    request["ScalableTargetAction"] = json!({"MinCapacity": 9});
    let (status, body) = emulator.call("PutScheduledAction", request).await;
    assert_eq!(
        status, 200,
        "case \"future action\": expected 200, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": NAMESPACE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"future action\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalableTargets"][0]["MinCapacity"],
        json!(1),
        "case \"future action\": expected the target's own bounds to be untouched, got {body:?}"
    );
}

#[tokio::test]
async fn a_scheduled_action_needs_a_registered_target() {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("orders", 1, 1).await;
    let (status, body) = emulator
        .call("PutScheduledAction", action("nightly", "rate(5 minutes)"))
        .await;
    assert_eq!(
        status, 400,
        "case \"unregistered target\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ObjectNotFoundException",
        "case \"unregistered target\": expected an ObjectNotFoundException, got {body:?}"
    );
    assert_eq!(
        error_message(&body),
        format!(
            "No scalable target found for service namespace: {NAMESPACE}, resource ID: \
             {RESOURCE_ID}, scalable dimension: {DIMENSION}"
        ),
        "case \"unregistered target\": expected the message to name the identity, got {body:?}"
    );

    // And deregistering the target takes its actions with it: "if a scalable target is
    // deregistered […] any scheduled actions that were specified for the scalable target
    // are deleted".
    let emulator = registered().await;
    let (status, body) = emulator
        .call("PutScheduledAction", action("nightly", "rate(5 minutes)"))
        .await;
    assert_eq!(
        status, 200,
        "case \"put before deregister\": expected 200, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call(
            "DeregisterScalableTarget",
            json!({"ServiceNamespace": NAMESPACE, "ResourceId": RESOURCE_ID,
                   "ScalableDimension": DIMENSION}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"deregister\": expected 200, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call(
            "DescribeScheduledActions",
            json!({"ServiceNamespace": NAMESPACE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"describe after deregister\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScheduledActions"],
        json!([]),
        "case \"describe after deregister\": expected the actions to be gone, got {body:?}"
    );
}
