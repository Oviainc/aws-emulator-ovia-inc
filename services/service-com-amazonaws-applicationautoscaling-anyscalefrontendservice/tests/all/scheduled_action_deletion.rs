// SPDX-License-Identifier: CC0-1.0

//! `DeleteScheduledAction`, end to end over the production router.
//!
//! The rules asserted here, each over a table of inputs rather than one call:
//!
//! * a scheduled action is identified by its scalable target (service namespace, resource
//!   id, scalable dimension) PLUS its name, so a delete removes exactly the action that
//!   identity names and leaves every other action — including one of the same name on
//!   another target — where it was;
//! * "for any operation that deletes or deregisters a resource, this exception is thrown if
//!   the resource cannot be found": a delete naming an action the store does not hold is
//!   refused with 400 `ObjectNotFoundException`, which is also what makes the call
//!   non-idempotent;
//! * the two identifying enums are validated against the model's own value sets before any
//!   state is touched, so a request carrying a value outside them, or a namespace that is
//!   not the one its dimension names, is refused with 400 `ValidationException` and deletes
//!   nothing;
//! * an occurrence the clock had already made due moved the target's bounds at the instant
//!   it was owed, so it is materialised before the action is removed rather than lost with
//!   it, while an occurrence still AHEAD of the delete never fires at all: the removal
//!   cancels it;
//! * a name at either published `ScheduledActionName` length bound round-trips through the
//!   delete, and one past the ceiling names no stored action.

use serde_json::{json, Value};

use super::emulator::{at_schedule_in, error_code, error_message, Emulator};

/// One row of the deletion catalog: the scalable dimension of the target the action hangs
/// off, and the schedule expression the action carries.
///
/// The dimensions are values of the model's own `ScalableDimension` enum
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#ScalableDimension")`),
/// one per family of resource-id form the harness names a resource for — a bare name, a
/// path with two segments, a table/index pair, a colon-qualified name, a full ARN — and the
/// schedules are the three forms the model documents for `Schedule` (`at`, `rate`, `cron`).
/// So the rows differ in the namespace, the resource-id form and the schedule form, none of
/// which the rule under test depends on.
struct Row {
    case: &'static str,
    dimension: &'static str,
    schedule: &'static str,
}

const CATALOG: &[Row] = &[
    Row {
        case: "a DynamoDB table's read capacity, scheduled by a rate",
        dimension: "dynamodb:table:ReadCapacityUnits",
        schedule: "rate(5 minutes)",
    },
    Row {
        case: "a DynamoDB index's write capacity, scheduled by a cron",
        dimension: "dynamodb:index:WriteCapacityUnits",
        schedule: "cron(0 10 * * ? *)",
    },
    Row {
        case: "an ECS service's desired count, scheduled once",
        dimension: "ecs:service:DesiredCount",
        schedule: "at(2035-06-01T12:00:00)",
    },
    Row {
        case: "a Lambda alias's provisioned concurrency",
        dimension: "lambda:function:ProvisionedConcurrency",
        schedule: "rate(2 hours)",
    },
    Row {
        case: "a custom resource's own property",
        dimension: "custom-resource:ResourceType:Property",
        schedule: "cron(30 9 ? * MON-FRI *)",
    },
    Row {
        case: "a Keyspaces table's read capacity",
        dimension: "cassandra:table:ReadCapacityUnits",
        schedule: "rate(1 day)",
    },
    Row {
        case: "an AppStream fleet's desired capacity",
        dimension: "appstream:fleet:DesiredCapacity",
        schedule: "at(2036-02-29T00:30:00)",
    },
    Row {
        case: "an MSK cluster's broker storage, named by an ARN",
        dimension: "kafka:broker-storage:VolumeSize",
        schedule: "cron(0 0 1 * ? *)",
    },
    Row {
        case: "a SageMaker variant's instance count",
        dimension: "sagemaker:variant:DesiredInstanceCount",
        schedule: "rate(15 minutes)",
    },
    Row {
        case: "a Spot Fleet request's target capacity",
        dimension: "ec2:spot-fleet-request:TargetCapacity",
        schedule: "rate(1 hour)",
    },
];

/// A `PutScheduledAction`/`DeleteScheduledAction` request for `name` against `target`,
/// where `target` is the three identity members [`Emulator::given_target`] answers.
fn request(target: &Value, name: &str) -> Value {
    let mut request = target.clone();
    request["ScheduledActionName"] = json!(name);
    request
}

/// A `DescribeScalableTargets` filter naming exactly `target`.
///
/// `DescribeScalableTargets` narrows by `ResourceIds` rather than by the single `ResourceId`
/// the identity carries, and a dimension filter sent with no resource id at all is refused —
/// a dimension names a capacity property of ONE resource — so the identity is spelled into
/// the members that operation reads.
fn describe_filter(target: &Value) -> Value {
    json!({
        "ServiceNamespace": target["ServiceNamespace"].clone(),
        "ResourceIds": [target["ResourceId"].clone()],
        "ScalableDimension": target["ScalableDimension"].clone(),
    })
}

/// Store a scheduled action called `name` on `target`.
async fn put(emulator: &Emulator, target: &Value, name: &str, schedule: &str, case: &str) {
    let mut body = request(target, name);
    body["Schedule"] = json!(schedule);
    let (status, response) = emulator.call("PutScheduledAction", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected PutScheduledAction for {name:?} to answer 200, got {status} \
         {response:?}"
    );
}

/// The names of the scheduled actions stored on `target`, in the order reported.
async fn names_on(emulator: &Emulator, target: &Value, case: &str) -> Vec<String> {
    let (status, body) = emulator
        .call("DescribeScheduledActions", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScheduledActions to answer 200, got {status} {body:?}"
    );
    body["ScheduledActions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|action| {
            action["ScheduledActionName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn deleting_an_action_removes_exactly_the_action_its_identity_names() {
    let emulator = Emulator::empty();
    for row in CATALOG {
        let target = emulator.given_target(row.dimension, 1, 10).await;
        put(&emulator, &target, "doomed", row.schedule, row.case).await;
        put(&emulator, &target, "kept", row.schedule, row.case).await;

        let (status, body) = emulator
            .call("DeleteScheduledAction", request(&target, "doomed"))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DeleteScheduledAction to answer 200, got {status} {body:?}",
            row.case
        );
        assert!(
            body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {:?}: expected an empty response, got {body:?}",
            row.case
        );

        // The action is gone and its sibling on the same target is not.
        let names = names_on(&emulator, &target, row.case).await;
        assert_eq!(
            names,
            vec!["kept".to_string()],
            "case {:?}: expected only the sibling action to survive, got {names:?}",
            row.case
        );

        // Deleting it again is refused: the caller can tell "it is gone now" from "that is
        // not its name".
        let (status, body) = emulator
            .call("DeleteScheduledAction", request(&target, "doomed"))
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected a second delete to answer 400, got {status} {body:?}",
            row.case
        );
        assert_eq!(
            error_code(&body),
            "ObjectNotFoundException",
            "case {:?}: expected an ObjectNotFoundException, got {body:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn an_action_name_is_scoped_to_one_scalable_target() {
    let emulator = Emulator::empty();
    // Four distinct identities carrying the SAME action name: two dimensions of one
    // resource, and the same two dimensions of another resource.
    let targets = [
        emulator
            .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
            .await,
        emulator
            .given_target("dynamodb:table:WriteCapacityUnits", 1, 10)
            .await,
        emulator
            .given_target("dynamodb:index:ReadCapacityUnits", 1, 10)
            .await,
        emulator.given_target("ecs:service:DesiredCount", 1, 10).await,
    ];
    for target in &targets {
        put(&emulator, target, "nightly", "rate(5 minutes)", "scoping").await;
    }

    for (index, deleted) in targets.iter().enumerate() {
        let case = format!("deleting nightly on target {index}");
        let (status, body) = emulator
            .call("DeleteScheduledAction", request(deleted, "nightly"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );

        // Every identity NOT yet deleted still holds its own action; every one already
        // deleted holds none.
        for (other, target) in targets.iter().enumerate() {
            let names = names_on(&emulator, target, &case).await;
            let expected: Vec<String> = if other <= index {
                Vec::new()
            } else {
                vec!["nightly".to_string()]
            };
            assert_eq!(
                names, expected,
                "case {case:?}: expected target {other} to hold {expected:?}, got {names:?}"
            );
        }
    }
}

/// What a refusal of a delete REPORTS, which follows which resource is missing: an action its
/// target does not hold, or the scalable target itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reports {
    /// The action asked for, by name, beside the target's own identifier.
    TheAction,
    /// The scalable target, by the triple the request addressed it with.
    TheTarget,
}

/// How a request can name an action the store does not hold, beyond simply never having
/// stored it: every row is one identity member off a stored action, so each asserts that
/// that member is part of the action's identity.
#[tokio::test]
async fn deleting_an_action_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let other_resource = emulator
        .given_target("dynamodb:index:ReadCapacityUnits", 1, 10)
        .await;
    let other_dimension = emulator
        .given_target("dynamodb:table:WriteCapacityUnits", 1, 10)
        .await;
    // Only the first target carries an action.
    put(&emulator, &target, "nightly", "rate(5 minutes)", "missing").await;

    let rows: Vec<(&str, Value, Reports)> = vec![
        (
            "a name that was never stored",
            request(&target, "weekly"),
            Reports::TheAction,
        ),
        (
            "the stored name on another resource id",
            request(&other_resource, "nightly"),
            Reports::TheAction,
        ),
        (
            "the stored name on another dimension of the same resource",
            request(&other_dimension, "nightly"),
            Reports::TheAction,
        ),
        (
            "the stored name in another letter case",
            request(&target, "Nightly"),
            Reports::TheAction,
        ),
        (
            "the stored name with a trailing space",
            request(&target, "nightly "),
            Reports::TheAction,
        ),
        (
            "a name on a target that was never registered",
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/never-registered",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "ScheduledActionName": "nightly",
            }),
            Reports::TheTarget,
        ),
    ];

    for (case, body, reports) in &rows {
        let (status, response) = emulator
            .call("DeleteScheduledAction", body.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ObjectNotFoundException",
            "case {case:?}: expected an ObjectNotFoundException, got {response:?}"
        );
        // A request whose scalable target IS registered is told about the action, by the name
        // it asked for; one whose target is not registered is told about the target, by the
        // triple that addressed it. The `aws_worded_client_errors` suite pins both sentences
        // in full.
        let message = error_message(&response);
        let named = match reports {
            Reports::TheAction => body["ScheduledActionName"].as_str().unwrap_or_default(),
            Reports::TheTarget => body["ResourceId"].as_str().unwrap_or_default(),
        };
        assert!(
            message.contains(named),
            "case {case:?}: expected the message to name {named:?}, got {message:?}"
        );
    }

    // None of those refusals removed the action they named one member off.
    let names = names_on(&emulator, &target, "after the refusals").await;
    assert_eq!(
        names,
        vec!["nightly".to_string()],
        "case \"after the refusals\": expected the stored action to survive, got {names:?}"
    );

    // And the one request that DOES name it succeeds, which is what makes the rows above
    // assertions about identity rather than about a store that refuses everything.
    let (status, body) = emulator
        .call("DeleteScheduledAction", request(&target, "nightly"))
        .await;
    assert_eq!(
        status, 200,
        "case \"the stored identity\": expected 200, got {status} {body:?}"
    );
}

/// The identifying enums a delete carries, checked against the model's own value sets
/// (`ServiceNamespace` and `ScalableDimension`). A value outside its enum is a bad member; a
/// dimension that belongs to ANOTHER namespace is a triple nothing registered a target for,
/// which this call reports as the missing target rather than as a bad member.
#[tokio::test]
async fn an_unmodelled_namespace_or_dimension_is_refused_and_deletes_nothing() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    put(&emulator, &target, "nightly", "rate(5 minutes)", "enums").await;

    for (case, member, value, expected) in [
        (
            "a namespace outside the enum",
            "ServiceNamespace",
            "dynamo",
            "ValidationException",
        ),
        (
            "an empty namespace",
            "ServiceNamespace",
            "",
            "ValidationException",
        ),
        (
            "a dimension outside the enum",
            "ScalableDimension",
            "dynamodb:table:Capacity",
            "ValidationException",
        ),
        (
            "an empty dimension",
            "ScalableDimension",
            "",
            "ValidationException",
        ),
        (
            "a dimension belonging to another namespace",
            "ScalableDimension",
            "ecs:service:DesiredCount",
            "ObjectNotFoundException",
        ),
    ] {
        let mut body = request(&target, "nightly");
        body[member] = json!(value);
        let (status, response) = emulator.call("DeleteScheduledAction", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            expected,
            "case {case:?}: expected a {expected}, got {response:?}"
        );
    }

    let names = names_on(&emulator, &target, "after the refusals").await;
    assert_eq!(
        names,
        vec!["nightly".to_string()],
        "case \"after the refusals\": expected the action to be untouched, got {names:?}"
    );
}

#[tokio::test]
async fn an_occurrence_already_due_is_applied_before_the_action_is_removed() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;

    // An `at` expression a couple of seconds out: once that instant passes the occurrence
    // was owed whether or not anybody called, so deleting the action afterwards must not
    // take the movement with it.
    let (schedule, at) = at_schedule_in(std::time::Duration::from_secs(2));
    let mut body = request(&target, "due-shortly");
    body["Schedule"] = json!(schedule);
    body["ScalableTargetAction"] = json!({"MinCapacity": 6, "MaxCapacity": 9});
    let (status, response) = emulator.call("PutScheduledAction", body).await;
    assert_eq!(
        status, 200,
        "case \"put\": expected 200, got {status} {response:?}"
    );

    // Nothing sleeps in the service: the due instant is stored and materialised by the
    // next request after it passes — here, the delete itself.
    let remaining = at
        .duration_since(std::time::SystemTime::now())
        .unwrap_or_default();
    tokio::time::sleep(remaining + std::time::Duration::from_millis(600)).await;

    let (status, response) = emulator
        .call("DeleteScheduledAction", request(&target, "due-shortly"))
        .await;
    assert_eq!(
        status, 200,
        "case \"delete\": expected 200, got {status} {response:?}"
    );
    let names = names_on(&emulator, &target, "delete").await;
    assert!(
        names.is_empty(),
        "case \"delete\": expected the action to be gone, got {names:?}"
    );

    let (status, response) = emulator
        .call("DescribeScalableTargets", describe_filter(&target))
        .await;
    assert_eq!(
        status, 200,
        "case \"describe the target\": expected 200, got {status} {response:?}"
    );
    let described = &response["ScalableTargets"][0];
    for (member, expected) in [("MinCapacity", 6), ("MaxCapacity", 9)] {
        assert_eq!(
            described[member],
            json!(expected),
            "case \"describe the target\": expected the due occurrence's {member} to survive the \
             delete, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_action_never_fires_an_occurrence_that_was_still_ahead() {
    let emulator = Emulator::empty();
    // Two dimensions of one table, each carrying the SAME action: the one on the read
    // dimension is deleted while its occurrence is still ahead, and the one on the write
    // dimension is the control that shows the occurrence did arrive.
    let deleted_from = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    let kept = emulator
        .given_target("dynamodb:table:WriteCapacityUnits", 1, 10)
        .await;

    let case = "an occurrence still ahead of the delete";
    let (schedule, at) = at_schedule_in(std::time::Duration::from_secs(5));
    for target in [&deleted_from, &kept] {
        let mut body = request(target, "due-shortly");
        body["Schedule"] = json!(schedule);
        body["ScalableTargetAction"] = json!({"MinCapacity": 6, "MaxCapacity": 30});
        let (status, response) = emulator.call("PutScheduledAction", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScheduledAction to answer 200, got {status} {response:?}"
        );
    }

    let (status, response) = emulator
        .call("DeleteScheduledAction", request(&deleted_from, "due-shortly"))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the delete to answer 200, got {status} {response:?}"
    );

    // Past the instant both actions were scheduled for.
    let remaining = at
        .duration_since(std::time::SystemTime::now())
        .unwrap_or_default();
    tokio::time::sleep(remaining + std::time::Duration::from_millis(1_500)).await;

    for (which, target, expected_min) in [
        ("the action that was kept", &kept, 6),
        ("the action that was deleted", &deleted_from, 1),
    ] {
        let (status, response) = emulator
            .call("DescribeScalableTargets", describe_filter(target))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} ({which}): expected DescribeScalableTargets to answer 200, got \
             {status} {response:?}"
        );
        let described = &response["ScalableTargets"][0];
        assert_eq!(
            described["MinCapacity"],
            json!(expected_min),
            "case {case:?} ({which}): expected MinCapacity {expected_min}, got {described:?}"
        );
    }

    // The movement that did not happen left no scaling activity behind for the deleted
    // action's target either.
    let (status, response) = emulator
        .call("DescribeScalingActivities", deleted_from.clone())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingActivities to answer 200, got {status} \
         {response:?}"
    );
    let activities = response["ScalingActivities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !activities.iter().any(|activity| {
            activity["Cause"]
                .as_str()
                .unwrap_or_default()
                .contains("due-shortly")
        }),
        "case {case:?}: expected no activity for the deleted action, got {activities:?}"
    );
}

/// The published bounds of the name a scheduled action can carry: `ScheduledActionName` is
/// `@length(min: 1, max: 256)`
/// (`smithy_query(query="shape",shape="com.amazonaws.applicationautoscaling#ScheduledActionName")`).
const MIN_NAME_CHARS: usize = 1;
/// See [`MIN_NAME_CHARS`].
const MAX_NAME_CHARS: usize = 256;

#[tokio::test]
async fn a_name_at_either_published_length_bound_is_deletable_and_one_past_it_is_not() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;

    for (case, chars) in [
        ("the shortest name", MIN_NAME_CHARS),
        ("one inside the ceiling", MAX_NAME_CHARS - 1),
        ("the longest name", MAX_NAME_CHARS),
    ] {
        let name = "n".repeat(chars);
        put(&emulator, &target, &name, "rate(5 minutes)", case).await;
        let (status, response) = emulator
            .call("DeleteScheduledAction", request(&target, &name))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} ({chars} characters): expected the delete to answer 200, got {status} \
             {response:?}"
        );
        let names = names_on(&emulator, &target, case).await;
        assert!(
            names.is_empty(),
            "case {case:?} ({chars} characters): expected the action to be gone, got {names:?}"
        );
    }

    // A name longer than any action can carry is still a legal `DeleteScheduledAction`
    // member — the request's own `ScheduledActionName` is bounded at 1600 — and it names no
    // stored action, so it is refused as a missing object rather than accepted.
    let case = "one past the ceiling";
    let name = "n".repeat(MAX_NAME_CHARS + 1);
    let (status, response) = emulator
        .call("DeleteScheduledAction", request(&target, &name))
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ObjectNotFoundException",
        "case {case:?}: expected the name to be refused as a missing object, got {response:?}"
    );
}
