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
//!   it.

use serde_json::{json, Value};

use super::emulator::{error_code, error_message, Emulator};

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

    let rows: Vec<(&str, Value)> = vec![
        ("a name that was never stored", request(&target, "weekly")),
        (
            "the stored name on another resource id",
            request(&other_resource, "nightly"),
        ),
        (
            "the stored name on another dimension of the same resource",
            request(&other_dimension, "nightly"),
        ),
        (
            "the stored name in another letter case",
            request(&target, "Nightly"),
        ),
        (
            "the stored name with a trailing space",
            request(&target, "nightly "),
        ),
        (
            "a name on a target that was never registered",
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/never-registered",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "ScheduledActionName": "nightly",
            }),
        ),
    ];

    for (case, body) in &rows {
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
        let message = error_message(&response);
        for member in ["ResourceId", "ScheduledActionName"] {
            let value = body[member].as_str().unwrap_or_default();
            assert!(
                message.contains(value),
                "case {case:?}: expected the message to name {member} ({value:?}), got \
                 {message:?}"
            );
        }
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
/// (`ServiceNamespace` and `ScalableDimension`), and against each other: a dimension names
/// the namespace it belongs to in its first segment.
#[tokio::test]
async fn an_unmodelled_namespace_or_dimension_is_refused_and_deletes_nothing() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;
    put(&emulator, &target, "nightly", "rate(5 minutes)", "enums").await;

    for (case, member, value) in [
        ("a namespace outside the enum", "ServiceNamespace", "dynamo"),
        ("an empty namespace", "ServiceNamespace", ""),
        (
            "a dimension outside the enum",
            "ScalableDimension",
            "dynamodb:table:Capacity",
        ),
        ("an empty dimension", "ScalableDimension", ""),
        (
            "a dimension belonging to another namespace",
            "ScalableDimension",
            "ecs:service:DesiredCount",
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
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response:?}"
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
    let at = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    let seconds = at
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs();
    let schedule = format!(
        "at({})",
        chrono::DateTime::from_timestamp(seconds as i64, 0)
            .expect("a representable instant")
            .format("%Y-%m-%dT%H:%M:%S")
    );
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
        .call("DescribeScalableTargets", target.clone())
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
