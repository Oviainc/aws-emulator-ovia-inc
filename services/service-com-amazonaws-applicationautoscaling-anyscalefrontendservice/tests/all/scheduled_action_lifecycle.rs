// SPDX-License-Identifier: CC0-1.0

//! The scheduled-action slice, end to end over the wire: `PutScheduledAction` and the
//! `DescribeScheduledActions` that reads back what it persisted.
//!
//! Every case drives the production `MultiServiceRouter` through the shared
//! [`Emulator`] harness the target/policy suite uses, so what is asserted is what a
//! signing AWS client would see — routing, deserialization, constraint validation, the
//! handler and serialization included.
//!
//! The suites are tables rather than single calls: each row varies the thing the rule
//! under test generalises over (the namespace/dimension pair, the schedule form, which
//! members a partial update names, which filter a Describe carries, the page size).

use serde_json::{json, Value};

use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::ops::put_scheduled_action::{
    SCHEDULE_CRON_FIELDS, SCHEDULE_RATE_UNITS,
};

use super::emulator::{error_code, Emulator, ACCOUNT, REGION};

/// The DynamoDB tables this suite's `dynamodb` rows register a scalable target on: a
/// target resolves the resource it scales in the service that owns it, so each table
/// exists before the target does.
const TABLES: &[&str] = &["my-table", "schedules", "updates", "alpha", "bravo", "bounds"];

/// Register `resource_id`/`dimension` so a scheduled action has a target to hang off,
/// creating the resource it scales where this emulator owns the service that resource
/// lives in.
async fn register(emulator: &Emulator, namespace: &str, resource_id: &str, dimension: &str) {
    emulator.given_resource(namespace, resource_id, 1).await;
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": namespace,
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
                "MinCapacity": 1,
                "MaxCapacity": 10,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "registering {resource_id:?}/{dimension:?}: expected 200, got {status} {body:?}"
    );
}

/// Every scheduled action `namespace` holds, in the order the service reports them,
/// walking every page so the result is the whole filtered set rather than one page.
async fn described(
    emulator: &Emulator,
    namespace: &str,
    filter: &Value,
    page_size: i64,
    case: &str,
) -> Vec<Value> {
    let mut actions = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(
            pages <= 64,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        let mut request = json!({"ServiceNamespace": namespace, "MaxResults": page_size});
        for (member, value) in filter.as_object().expect("the filter is an object") {
            request[member] = value.clone();
        }
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = emulator.call("DescribeScheduledActions", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScheduledActions to answer 200, got {status} {body:?}"
        );
        let page = body["ScheduledActions"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: expected a ScheduledActions list, got {body:?}")
        });
        assert!(
            page.len() <= page_size as usize,
            "case {case:?}: expected at most {page_size} actions per page, got {page:?}"
        );
        actions.extend(page.iter().cloned());
        next_token = body
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if next_token.is_none() {
            break;
        }
    }
    actions
}

/// The single scheduled action a filter selects, for a case that stored exactly one.
async fn only_action(emulator: &Emulator, namespace: &str, filter: &Value, case: &str) -> Value {
    let actions = described(emulator, namespace, filter, 50, case).await;
    assert_eq!(
        actions.len(),
        1,
        "case {case:?}: expected exactly one scheduled action for {filter:?}, got {actions:?}"
    );
    actions[0].clone()
}

/// A row of the round-trip catalog.
///
/// The namespace/dimension pairs come from the model's own `ServiceNamespace` and
/// `ScalableDimension` value sets; the schedule forms are the three the model documents
/// for `Schedule` (at / rate / cron). The rows differ in the schedule form, whether a
/// `Timezone` is named, whether the recurrence is bounded by start/end times, and whether
/// the action sets both capacity sides, only the floor or only the ceiling.
struct PutRow {
    case: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    schedule: &'static str,
    timezone: Option<&'static str>,
    start_time: Option<i64>,
    end_time: Option<i64>,
    min_capacity: Option<i64>,
    max_capacity: Option<i64>,
}

const PUT_CATALOG: &[PutRow] = &[
    PutRow {
        case: "a one-off at expression on a DynamoDB table",
        namespace: "dynamodb",
        resource_id: "table/my-table",
        dimension: "dynamodb:table:ReadCapacityUnits",
        schedule: "at(2030-01-01T00:00:00)",
        timezone: None,
        start_time: None,
        end_time: None,
        min_capacity: Some(5),
        max_capacity: Some(20),
    },
    PutRow {
        case: "a bounded rate expression on an ECS service",
        namespace: "ecs",
        resource_id: "service/my-cluster/my-service",
        dimension: "ecs:service:DesiredCount",
        schedule: "rate(30 minutes)",
        timezone: None,
        start_time: Some(1_900_000_000),
        end_time: Some(1_900_600_000),
        min_capacity: Some(2),
        max_capacity: Some(2),
    },
    PutRow {
        case: "a cron expression in a named time zone",
        namespace: "lambda",
        resource_id: "function:my-function:provisioned",
        dimension: "lambda:function:ProvisionedConcurrency",
        schedule: "cron(0 9 * * ? *)",
        timezone: Some("Pacific/Tahiti"),
        start_time: Some(1_900_000_000),
        end_time: None,
        min_capacity: Some(0),
        max_capacity: None,
    },
    PutRow {
        case: "a ceiling-only action on an Aurora cluster",
        namespace: "rds",
        resource_id: "cluster:my-db-cluster",
        dimension: "rds:cluster:ReadReplicaCount",
        schedule: "rate(1 day)",
        timezone: Some("Etc/GMT+9"),
        start_time: None,
        end_time: None,
        min_capacity: None,
        max_capacity: Some(3),
    },
    PutRow {
        case: "a schedule with no capacity window at all",
        namespace: "appstream",
        resource_id: "fleet/sample-fleet",
        dimension: "appstream:fleet:DesiredCapacity",
        schedule: "at(2031-12-31T23:59:59)",
        timezone: None,
        start_time: None,
        end_time: None,
        min_capacity: None,
        max_capacity: None,
    },
];

/// The `PutScheduledAction` request a row describes.
fn put_request(row: &PutRow, name: &str) -> Value {
    let mut request = json!({
        "ServiceNamespace": row.namespace,
        "ResourceId": row.resource_id,
        "ScalableDimension": row.dimension,
        "ScheduledActionName": name,
        "Schedule": row.schedule,
    });
    if let Some(timezone) = row.timezone {
        request["Timezone"] = json!(timezone);
    }
    if let Some(start_time) = row.start_time {
        request["StartTime"] = json!(start_time);
    }
    if let Some(end_time) = row.end_time {
        request["EndTime"] = json!(end_time);
    }
    let mut action = json!({});
    if let Some(min_capacity) = row.min_capacity {
        action["MinCapacity"] = json!(min_capacity);
    }
    if let Some(max_capacity) = row.max_capacity {
        action["MaxCapacity"] = json!(max_capacity);
    }
    if action.as_object().is_some_and(|members| !members.is_empty()) {
        request["ScalableTargetAction"] = action;
    }
    request
}

#[tokio::test]
async fn a_scheduled_action_round_trips_through_describe() {
    let emulator = Emulator::with_tables(TABLES).await;
    let name = "scale-up";

    for row in PUT_CATALOG {
        let case = row.case;
        register(&emulator, row.namespace, row.resource_id, row.dimension).await;

        let (status, body) = emulator
            .call("PutScheduledAction", put_request(row, name))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScheduledAction to answer 200, got {status} {body:?}"
        );
        // The modelled output carries no members, so the only thing a Put reports is
        // success; everything it did is read back below.
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: expected an empty PutScheduledAction response, got {body:?}"
        );

        let filter = json!({"ResourceId": row.resource_id, "ScalableDimension": row.dimension});
        let action = only_action(&emulator, row.namespace, &filter, case).await;

        assert_eq!(
            action["ScheduledActionName"],
            json!(name),
            "case {case:?}: expected the name it was put under, got {action:?}"
        );
        assert_eq!(
            action["ServiceNamespace"],
            json!(row.namespace),
            "case {case:?}: expected the namespace it was put under, got {action:?}"
        );
        assert_eq!(
            action["ResourceId"],
            json!(row.resource_id),
            "case {case:?}: expected the resource id it was put under, got {action:?}"
        );
        assert_eq!(
            action["ScalableDimension"],
            json!(row.dimension),
            "case {case:?}: expected the dimension it was put under, got {action:?}"
        );
        assert_eq!(
            action["Schedule"],
            json!(row.schedule),
            "case {case:?}: expected the schedule it was put with, got {action:?}"
        );
        assert_eq!(
            action.get("Timezone").and_then(Value::as_str),
            row.timezone,
            "case {case:?}: expected Timezone {:?}, got {action:?}",
            row.timezone
        );
        assert_eq!(
            action.get("StartTime").and_then(Value::as_f64),
            row.start_time.map(|at| at as f64),
            "case {case:?}: expected StartTime {:?}, got {action:?}",
            row.start_time
        );
        assert_eq!(
            action.get("EndTime").and_then(Value::as_f64),
            row.end_time.map(|at| at as f64),
            "case {case:?}: expected EndTime {:?}, got {action:?}",
            row.end_time
        );
        assert_eq!(
            action["ScalableTargetAction"]
                .get("MinCapacity")
                .and_then(Value::as_i64),
            row.min_capacity,
            "case {case:?}: expected MinCapacity {:?}, got {action:?}",
            row.min_capacity
        );
        assert_eq!(
            action["ScalableTargetAction"]
                .get("MaxCapacity")
                .and_then(Value::as_i64),
            row.max_capacity,
            "case {case:?}: expected MaxCapacity {:?}, got {action:?}",
            row.max_capacity
        );

        // The ARN is the one Application Auto Scaling documents for a scheduled action,
        // and it names the resource and the action it belongs to.
        let arn = action["ScheduledActionARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ScheduledActionARN, got {action:?}"))
            .to_string();
        let expected_prefix = format!("arn:aws:autoscaling:{REGION}:{ACCOUNT}:scheduledAction:");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected a ScheduledActionARN under {expected_prefix:?}, got {arn:?}"
        );
        assert!(
            arn.ends_with(&format!(
                ":resource/{}/{}:scheduledActionName/{name}",
                row.namespace, row.resource_id
            )),
            "case {case:?}: expected the ARN to name its resource and action, got {arn:?}"
        );
        let creation_time = action["CreationTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a numeric CreationTime, got {action:?}")
        });
        assert!(
            creation_time > 0.0,
            "case {case:?}: expected a CreationTime after the epoch, got {creation_time}"
        );

        // A read must not change what the next reader sees.
        let again = only_action(&emulator, row.namespace, &filter, case).await;
        assert_eq!(
            again, action,
            "case {case:?}: a second Describe reported something else: {again:?}"
        );

        // Re-putting the same request is an upsert on the same identity: one action
        // still, with the ARN and the creation time it was minted with.
        let (status, body) = emulator
            .call("PutScheduledAction", put_request(row, name))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-Put to answer 200, got {status} {body:?}"
        );
        let upserted = only_action(&emulator, row.namespace, &filter, case).await;
        assert_eq!(
            upserted, action,
            "case {case:?}: expected the re-Put to leave the action unchanged, got {upserted:?}"
        );
    }
}

#[tokio::test]
async fn only_the_documented_schedule_forms_are_accepted() {
    let emulator = Emulator::with_tables(TABLES).await;
    let namespace = "dynamodb";
    let resource_id = "table/schedules";
    let dimension = "dynamodb:table:WriteCapacityUnits";
    register(&emulator, namespace, resource_id, dimension).await;

    // Every unit the model's `Schedule` documentation lists, tested as a rate expression
    // rather than only the one the recorded case used.
    let mut accepted: Vec<String> = SCHEDULE_RATE_UNITS
        .iter()
        .map(|(unit, _seconds)| format!("rate(1 {unit})"))
        .collect();
    accepted.extend(
        [
            "rate(12 hours)",
            "at(2032-02-29T12:00:00)",
            "at(1970-01-01T00:00:00)",
            // Six fields: [Minutes] [Hours] [Day_of_Month] [Month] [Day_of_Week] [Year].
            "cron(0 9 * * ? *)",
            "cron(15 */2 1 * ? 2030)",
        ]
        .iter()
        .map(ToString::to_string),
    );
    assert_eq!(
        "0 9 * * ? *".split_whitespace().count(),
        SCHEDULE_CRON_FIELDS,
        "the accepted cron row must carry the documented field count"
    );

    for (index, schedule) in accepted.iter().enumerate() {
        let name = format!("accepted-{index}");
        let (status, body) = emulator
            .call(
                "PutScheduledAction",
                json!({
                    "ServiceNamespace": namespace,
                    "ResourceId": resource_id,
                    "ScalableDimension": dimension,
                    "ScheduledActionName": name,
                    "Schedule": schedule,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {schedule:?}: expected 200, got {status} {body:?}"
        );
        let action = only_action(
            &emulator,
            namespace,
            &json!({"ScheduledActionNames": [name.clone()]}),
            schedule,
        )
        .await;
        assert_eq!(
            action["Schedule"],
            json!(schedule),
            "case {schedule:?}: expected the schedule to be stored verbatim, got {action:?}"
        );
    }

    // A form the documentation does not describe is refused, and persists nothing.
    let rejected: &[(&str, &str)] = &[
        ("an unknown rate unit", "rate(5 fortnights)"),
        ("a plural-only unit misspelling", "rate(5 minutess)"),
        ("a zero rate value", "rate(0 minutes)"),
        ("a negative rate value", "rate(-5 minutes)"),
        ("a fractional rate value", "rate(1.5 hours)"),
        ("a rate with no unit", "rate(5)"),
        ("a rate with extra words", "rate(5 minutes please)"),
        ("an unknown expression kind", "every(5 minutes)"),
        ("a bare timestamp", "2030-01-01T00:00:00"),
        ("an unclosed expression", "rate(5 minutes"),
        ("an at expression with a zone", "at(2030-01-01T00:00:00Z)"),
        ("an at expression with a date that does not exist", "at(2030-02-30T00:00:00)"),
        ("February 29th in a year that has none", "at(2030-02-29T00:00:00)"),
        ("an at expression with an hour out of range", "at(2030-01-01T25:00:00)"),
        ("a cron expression with five fields", "cron(0 9 * * ?)"),
        ("a cron expression with seven fields", "cron(0 0 9 * * ? 2030)"),
        ("an empty expression", ""),
    ];

    for (case, schedule) in rejected {
        let name = format!("rejected-{case}");
        let (status, body) = emulator
            .call(
                "PutScheduledAction",
                json!({
                    "ServiceNamespace": namespace,
                    "ResourceId": resource_id,
                    "ScalableDimension": dimension,
                    "ScheduledActionName": name,
                    "Schedule": schedule,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({schedule:?}): expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?} ({schedule:?}): expected a ValidationException, got {body:?}"
        );
        let actions = described(
            &emulator,
            namespace,
            &json!({"ScheduledActionNames": [name.clone()]}),
            50,
            case,
        )
        .await;
        assert!(
            actions.is_empty(),
            "case {case:?} ({schedule:?}): a refused Put must persist nothing, got {actions:?}"
        );
    }
}

/// A row of the partial-update table: which members the second Put names, and what the
/// stored action must look like afterwards.
struct UpdateRow {
    case: &'static str,
    request: fn() -> Value,
    schedule: &'static str,
    timezone: Option<&'static str>,
    start_time: Option<i64>,
    end_time: Option<i64>,
    min_capacity: Option<i64>,
    max_capacity: Option<i64>,
}

#[tokio::test]
async fn a_partial_update_keeps_what_it_does_not_name_except_the_times() {
    let emulator = Emulator::with_tables(TABLES).await;
    let namespace = "dynamodb";
    let resource_id = "table/updates";
    let dimension = "dynamodb:table:ReadCapacityUnits";
    let name = "nightly";
    register(&emulator, namespace, resource_id, dimension).await;

    // The baseline every row updates: a schedule, a time zone, both boundaries and both
    // capacity sides, so a row can be seen to keep or to replace each of them.
    let baseline = json!({
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": dimension,
        "ScheduledActionName": name,
        "Schedule": "cron(0 9 * * ? *)",
        "Timezone": "Etc/GMT+9",
        "StartTime": 1_900_000_000,
        "EndTime": 1_900_600_000,
        "ScalableTargetAction": {"MinCapacity": 4, "MaxCapacity": 40},
    });

    let rows: &[UpdateRow] = &[
        UpdateRow {
            case: "the action alone: the schedule survives, the times are deleted",
            request: || json!({"ScalableTargetAction": {"MinCapacity": 7, "MaxCapacity": 70}}),
            schedule: "cron(0 9 * * ? *)",
            timezone: Some("Etc/GMT+9"),
            start_time: None,
            end_time: None,
            min_capacity: Some(7),
            max_capacity: Some(70),
        },
        UpdateRow {
            case: "a ceiling-only action replaces the stored window rather than merging",
            request: || json!({"ScalableTargetAction": {"MaxCapacity": 9}}),
            schedule: "cron(0 9 * * ? *)",
            timezone: Some("Etc/GMT+9"),
            start_time: None,
            end_time: None,
            min_capacity: None,
            max_capacity: Some(9),
        },
        UpdateRow {
            case: "the schedule alone: the action survives, the times are deleted",
            request: || json!({"Schedule": "rate(6 hours)"}),
            schedule: "rate(6 hours)",
            timezone: Some("Etc/GMT+9"),
            start_time: None,
            end_time: None,
            min_capacity: Some(4),
            max_capacity: Some(40),
        },
        UpdateRow {
            case: "a new time zone, keeping the schedule it qualifies",
            request: || json!({"Timezone": "Pacific/Tahiti"}),
            schedule: "cron(0 9 * * ? *)",
            timezone: Some("Pacific/Tahiti"),
            start_time: None,
            end_time: None,
            min_capacity: Some(4),
            max_capacity: Some(40),
        },
        UpdateRow {
            case: "both boundaries restated, so both survive",
            request: || json!({"StartTime": 1_910_000_000, "EndTime": 1_920_000_000}),
            schedule: "cron(0 9 * * ? *)",
            timezone: Some("Etc/GMT+9"),
            start_time: Some(1_910_000_000),
            end_time: Some(1_920_000_000),
            min_capacity: Some(4),
            max_capacity: Some(40),
        },
        UpdateRow {
            case: "only the start restated, so the end is deleted",
            request: || json!({"StartTime": 1_910_000_000}),
            schedule: "cron(0 9 * * ? *)",
            timezone: Some("Etc/GMT+9"),
            start_time: Some(1_910_000_000),
            end_time: None,
            min_capacity: Some(4),
            max_capacity: Some(40),
        },
    ];

    for row in rows {
        let case = row.case;
        // Restore the baseline before each row, so rows do not depend on each other.
        let (status, body) = emulator
            .call("PutScheduledAction", baseline.clone())
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the baseline Put to answer 200, got {status} {body:?}"
        );
        let stored = only_action(
            &emulator,
            namespace,
            &json!({"ScheduledActionNames": [name]}),
            case,
        )
        .await;
        let baseline_arn = stored["ScheduledActionARN"].clone();
        let baseline_creation = stored["CreationTime"].clone();

        let mut request = json!({
            "ServiceNamespace": namespace,
            "ResourceId": resource_id,
            "ScalableDimension": dimension,
            "ScheduledActionName": name,
        });
        for (member, value) in (row.request)()
            .as_object()
            .expect("the update is an object")
        {
            request[member] = value.clone();
        }
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to answer 200, got {status} {body:?}"
        );

        let action = only_action(
            &emulator,
            namespace,
            &json!({"ScheduledActionNames": [name]}),
            case,
        )
        .await;
        assert_eq!(
            action["Schedule"],
            json!(row.schedule),
            "case {case:?}: expected Schedule {:?}, got {action:?}",
            row.schedule
        );
        assert_eq!(
            action.get("Timezone").and_then(Value::as_str),
            row.timezone,
            "case {case:?}: expected Timezone {:?}, got {action:?}",
            row.timezone
        );
        assert_eq!(
            action.get("StartTime").and_then(Value::as_f64),
            row.start_time.map(|at| at as f64),
            "case {case:?}: expected StartTime {:?}, got {action:?}",
            row.start_time
        );
        assert_eq!(
            action.get("EndTime").and_then(Value::as_f64),
            row.end_time.map(|at| at as f64),
            "case {case:?}: expected EndTime {:?}, got {action:?}",
            row.end_time
        );
        assert_eq!(
            action["ScalableTargetAction"]
                .get("MinCapacity")
                .and_then(Value::as_i64),
            row.min_capacity,
            "case {case:?}: expected MinCapacity {:?}, got {action:?}",
            row.min_capacity
        );
        assert_eq!(
            action["ScalableTargetAction"]
                .get("MaxCapacity")
                .and_then(Value::as_i64),
            row.max_capacity,
            "case {case:?}: expected MaxCapacity {:?}, got {action:?}",
            row.max_capacity
        );
        assert_eq!(
            action["ScheduledActionARN"], baseline_arn,
            "case {case:?}: expected the ARN to survive the update, got {action:?}"
        );
        assert_eq!(
            action["CreationTime"], baseline_creation,
            "case {case:?}: expected the CreationTime to survive the update, got {action:?}"
        );
    }
}

#[tokio::test]
async fn a_scheduled_action_name_is_scoped_to_its_target() {
    let emulator = Emulator::with_tables(TABLES).await;
    let namespace = "dynamodb";
    let resources = ["table/alpha", "table/bravo"];
    let dimensions = [
        "dynamodb:table:ReadCapacityUnits",
        "dynamodb:table:WriteCapacityUnits",
    ];
    let names = ["morning", "evening"];

    // The same two names on each of the four targets: eight distinct actions, because a
    // name identifies an action only together with its namespace, resource and dimension.
    for resource_id in resources {
        for dimension in dimensions {
            register(&emulator, namespace, resource_id, dimension).await;
            for name in names {
                let (status, body) = emulator
                    .call(
                        "PutScheduledAction",
                        json!({
                            "ServiceNamespace": namespace,
                            "ResourceId": resource_id,
                            "ScalableDimension": dimension,
                            "ScheduledActionName": name,
                            "Schedule": "rate(1 hour)",
                            "ScalableTargetAction": {"MinCapacity": 1, "MaxCapacity": 5},
                        }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {resource_id:?}/{dimension:?}/{name:?}: expected 200, got {status} {body:?}"
                );
            }
        }
    }

    // Each filter combination, and how many of the eight stored actions it selects. The
    // filters intersect: a name list narrows within the resource and dimension, never
    // beyond them.
    let rows: &[(&str, Value, usize)] = &[
        ("no filter", json!({}), 8),
        (
            "by resource id",
            json!({"ResourceId": "table/alpha"}),
            4,
        ),
        (
            "by resource id and dimension",
            json!({"ResourceId": "table/bravo", "ScalableDimension": "dynamodb:table:ReadCapacityUnits"}),
            2,
        ),
        (
            "by name",
            json!({"ScheduledActionNames": ["morning"]}),
            4,
        ),
        (
            "by name and resource id",
            json!({"ResourceId": "table/alpha", "ScheduledActionNames": ["morning"]}),
            2,
        ),
        (
            "by name, resource id and dimension",
            json!({"ResourceId": "table/alpha", "ScalableDimension": "dynamodb:table:WriteCapacityUnits", "ScheduledActionNames": ["evening"]}),
            1,
        ),
        (
            "by both names",
            json!({"ScheduledActionNames": ["morning", "evening"]}),
            8,
        ),
        ("by an empty name list", json!({"ScheduledActionNames": []}), 8),
        (
            "by a name no action carries",
            json!({"ScheduledActionNames": ["absent"]}),
            0,
        ),
        (
            "by a resource no target carries",
            json!({"ResourceId": "table/absent"}),
            0,
        ),
    ];

    // Every row is walked at several page sizes, so the count is of the filtered set and
    // pagination neither drops nor repeats an action.
    for (case, filter, expected) in rows {
        for page_size in [1, 3, 50] {
            let case = format!("{case} at page size {page_size}");
            let actions = described(&emulator, namespace, filter, page_size, &case).await;
            assert_eq!(
                actions.len(),
                *expected,
                "case {case:?}: expected {expected} scheduled actions, got {actions:?}"
            );
            let mut identities: Vec<(String, String, String)> = actions
                .iter()
                .map(|action| {
                    (
                        action["ResourceId"].as_str().unwrap_or_default().to_string(),
                        action["ScalableDimension"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        action["ScheduledActionName"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect();
            let reported = identities.clone();
            identities.sort();
            identities.dedup();
            assert_eq!(
                identities.len(),
                reported.len(),
                "case {case:?}: expected every action once, got {reported:?}"
            );
            assert_eq!(
                reported,
                {
                    let mut sorted = reported.clone();
                    sorted.sort();
                    sorted
                },
                "case {case:?}: expected a stable order across pages, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_scheduled_action_needs_a_registered_target_and_a_matching_dimension() {
    let emulator = Emulator::with_tables(TABLES).await;

    // An action on a target nobody registered: "You cannot create a scheduled action
    // until you have registered the resource as a scalable target."
    let unregistered: &[(&str, &str, &str)] = &[
        ("dynamodb", "table/never-registered", "dynamodb:table:ReadCapacityUnits"),
        ("ecs", "service/my-cluster/absent", "ecs:service:DesiredCount"),
        ("kafka", "arn:aws:kafka:us-east-1:000000000000:cluster/absent", "kafka:broker-storage:VolumeSize"),
    ];
    for (namespace, resource_id, dimension) in unregistered {
        let (status, body) = emulator
            .call(
                "PutScheduledAction",
                json!({
                    "ServiceNamespace": namespace,
                    "ResourceId": resource_id,
                    "ScalableDimension": dimension,
                    "ScheduledActionName": "orphan",
                    "Schedule": "rate(1 hour)",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {resource_id:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ObjectNotFoundException"),
            "case {resource_id:?}: expected an ObjectNotFoundException, got {body:?}"
        );
    }

    // A namespace and a dimension that are each legal but do not belong together: nothing
    // could have registered that pair, so the triple names no scalable target to hang the
    // action off — which is what this call reports, unless the `ResourceId` does not take the
    // form the namespace documents for the dimension's resource type, which is a bad request
    // and is refused as one first.
    let mismatches: &[(&str, &str, &str, &str)] = &[
        (
            "a dimension of another namespace, whose resource type that namespace documents no form for",
            "ecs",
            "dynamodb:table:ReadCapacityUnits",
            "ObjectNotFoundException",
        ),
        (
            "a dimension of another namespace, whose resource type this one documents a form for",
            "neptune",
            "rds:cluster:ReadReplicaCount",
            "ValidationException",
        ),
    ];
    for (case, namespace, dimension, expected) in mismatches {
        let (status, body) = emulator
            .call(
                "PutScheduledAction",
                json!({
                    "ServiceNamespace": namespace,
                    "ResourceId": "res/one",
                    "ScalableDimension": dimension,
                    "ScheduledActionName": "mismatched",
                    "Schedule": "rate(1 hour)",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains(expected),
            "case {case:?}: expected a {expected}, got {body:?}"
        );
    }

    // A capacity window that is not a window, and a boundary pair in the wrong order.
    let namespace = "dynamodb";
    let resource_id = "table/bounds";
    let dimension = "dynamodb:table:ReadCapacityUnits";
    register(&emulator, namespace, resource_id, dimension).await;
    let bad_windows: &[(&str, Value)] = &[
        (
            "a floor above the ceiling",
            json!({"ScalableTargetAction": {"MinCapacity": 9, "MaxCapacity": 2}}),
        ),
        (
            "a negative floor",
            json!({"ScalableTargetAction": {"MinCapacity": -1}}),
        ),
        (
            "a negative ceiling",
            json!({"ScalableTargetAction": {"MaxCapacity": -5}}),
        ),
        (
            "an end before the start",
            json!({"StartTime": 1_900_600_000, "EndTime": 1_900_000_000}),
        ),
    ];
    for (case, extra) in bad_windows {
        let mut request = json!({
            "ServiceNamespace": namespace,
            "ResourceId": resource_id,
            "ScalableDimension": dimension,
            "ScheduledActionName": "bounded",
            "Schedule": "rate(1 hour)",
        });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            request[member] = value.clone();
        }
        let (status, body) = emulator.call("PutScheduledAction", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    // A window OUTSIDE the registered range is stored: it is measured against the
    // target when the action runs, not when it is put.
    let (status, body) = emulator
        .call(
            "PutScheduledAction",
            json!({
                "ServiceNamespace": namespace,
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
                "ScheduledActionName": "above-the-registered-maximum",
                "Schedule": "rate(1 hour)",
                "ScalableTargetAction": {"MinCapacity": 400, "MaxCapacity": 500},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"above the registered maximum\": expected 200, got {status} {body:?}"
    );

    // Deregistering the target deletes the scheduled actions that hang off it.
    let (status, body) = emulator
        .call(
            "DeregisterScalableTarget",
            json!({
                "ServiceNamespace": namespace,
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"deregister\": expected 200, got {status} {body:?}"
    );
    let actions = described(&emulator, namespace, &json!({}), 50, "after deregister").await;
    assert!(
        actions.is_empty(),
        "case \"after deregister\": expected the cascade to delete every action, got {actions:?}"
    );
}

#[tokio::test]
async fn describe_refuses_a_filter_it_cannot_apply() {
    let emulator = Emulator::with_tables(TABLES).await;

    // "If you specify a scalable dimension, you must also specify a resource ID."
    for dimension in [
        "dynamodb:table:ReadCapacityUnits",
        "dynamodb:index:WriteCapacityUnits",
    ] {
        let (status, body) = emulator
            .call(
                "DescribeScheduledActions",
                json!({"ServiceNamespace": "dynamodb", "ScalableDimension": dimension}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {dimension:?}: expected HTTP 400 without a ResourceId, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {dimension:?}: expected a ValidationException, got {body:?}"
        );

        // The same dimension WITH a resource id is a filter the service can apply.
        let (status, body) = emulator
            .call(
                "DescribeScheduledActions",
                json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/any", "ScalableDimension": dimension}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected 200 with a ResourceId, got {status} {body:?}"
        );
    }

    // `ScheduledActionNames` is bounded at 0..50 by the model: the published bound is
    // accepted and one past it is refused.
    for (case, count, expected_status) in [
        ("the upper bound", 50_usize, 200_u16),
        ("one past the upper bound", 51, 400),
    ] {
        let names: Vec<String> = (0..count).map(|index| format!("name-{index}")).collect();
        let (status, body) = emulator
            .call(
                "DescribeScheduledActions",
                json!({"ServiceNamespace": "dynamodb", "ScheduledActionNames": names}),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({count} names): expected HTTP {expected_status}, got {status} {body:?}"
        );
        if expected_status == 400 {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
        }
    }

    // `MaxResults` "can be between 1 and 50": both published bounds are accepted and one
    // past each is refused.
    for (case, max_results, expected_status) in [
        ("the lower bound", 1_i64, 200_u16),
        ("the upper bound", 50, 200),
        ("one below the lower bound", 0, 400),
        ("one past the upper bound", 51, 400),
    ] {
        let (status, body) = emulator
            .call(
                "DescribeScheduledActions",
                json!({"ServiceNamespace": "dynamodb", "MaxResults": max_results}),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body:?}"
        );
    }

    // A token this service did not issue is not a token.
    for (case, token) in [
        ("plain garbage", "garbage"),
        ("a non-hex payload", "not-hex-at-all!!"),
        ("hex carrying too few fields", "6162"),
        ("an empty token", ""),
    ] {
        let (status, body) = emulator
            .call(
                "DescribeScheduledActions",
                json!({"ServiceNamespace": "dynamodb", "NextToken": token}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("InvalidNextTokenException"),
            "case {case:?}: expected an InvalidNextTokenException, got {body:?}"
        );
    }

    // An unknown namespace is refused by the enum check, not answered with an empty list.
    let (status, body) = emulator
        .call(
            "DescribeScheduledActions",
            json!({"ServiceNamespace": "not-a-namespace"}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"unknown namespace\": expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case \"unknown namespace\": expected a ValidationException, got {body:?}"
    );
}
