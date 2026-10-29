// SPDX-License-Identifier: CC0-1.0

//! What a registration does to the RESOURCE it registers, and what a report of a scalable
//! target says, end to end over the wire.
//!
//! Four rules, each asserted over a table rather than over one call:
//!
//! * a registration enforces the capacity window ON THE RESOURCE: "Application Auto Scaling
//!   retrieves the current capacity of the resource. If it's below the minimum capacity or
//!   above the maximum capacity, Application Auto Scaling adjusts the capacity of the
//!   scalable target to place it within these bounds, even if you don't include the
//!   MinCapacity or MaxCapacity request parameters" — so the capacity a client reads back in
//!   the service that OWNS the resource is inside the window, on a first registration as on
//!   an update;
//! * every capacity movement the log reports is one the resource shows, and a registration
//!   that moves nothing reports nothing: the activity and the movement are the same event;
//! * a scalable target is a capacity dimension of a resource, so a registration against a
//!   resource the owning service does not hold is refused, and that refusal persists
//!   nothing;
//! * `ScalableTarget.SuspendedState` carries all three flags the shape declares, whatever
//!   the registration said about them.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use super::emulator::{error_code, Emulator};

/// How long a case waits before reading the resource a movement was queued for. It exceeds
/// the fulfilment span the service gives a movement (600 ms), and nothing sleeps for it
/// inside the emulator.
const FULFILMENT_WAIT: Duration = Duration::from_millis(900);

/// Which service owns the resource a row scales, and therefore where its capacity is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    /// A PROVISIONED DynamoDB table's own read or write capacity units.
    DynamoDbTable,
    /// A PROVISIONED table's global secondary index capacity units.
    DynamoDbIndex,
    /// An ECS service's desired count.
    EcsService,
}

/// One enforcement case: a resource at a capacity, a window registered over it, and the
/// capacity the resource must be left at.
struct WindowRow {
    case: &'static str,
    owner: Owner,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    /// The capacity the resource is created with.
    starting: i64,
    /// The window the FIRST registration declares.
    first: (i64, i64),
    /// The members a second registration names, or `None` for a case that registers once.
    update: Option<fn() -> Value>,
    /// The capacity the resource must hold once the registrations are done.
    expected: i64,
    /// The `Description` of every activity the log must report, newest first; empty for a
    /// pair of registrations that moved nothing.
    expected_activities: &'static [&'static str],
}

/// The catalog. The rows differ along what the rule generalises over: which service owns the
/// scaled resource and which property of it the dimension names, whether the capacity is
/// corrected UP by the floor or DOWN by the ceiling, whether the correction happens on the
/// first registration or on a later one, and the magnitude of the correction. The first row
/// is the case the issue cited: a table at one read unit registered with a floor of five.
const WINDOW_CATALOG: &[WindowRow] = &[
    WindowRow {
        case: "a floor above the table's read capacity, on the first registration",
        owner: Owner::DynamoDbTable,
        namespace: "dynamodb",
        resource_id: "table/catalog",
        dimension: "dynamodb:table:ReadCapacityUnits",
        starting: 1,
        first: (5, 10),
        update: None,
        expected: 5,
        expected_activities: &["Setting read capacity units to 5."],
    },
    WindowRow {
        case: "a floor raised above the table's write capacity by an update",
        owner: Owner::DynamoDbTable,
        namespace: "dynamodb",
        resource_id: "table/catalog",
        dimension: "dynamodb:table:WriteCapacityUnits",
        starting: 2,
        first: (1, 40),
        update: Some(|| json!({"MinCapacity": 30})),
        expected: 30,
        expected_activities: &["Setting write capacity units to 30."],
    },
    WindowRow {
        case: "a ceiling below the table's read capacity moves it down",
        owner: Owner::DynamoDbTable,
        namespace: "dynamodb",
        resource_id: "table/catalog",
        dimension: "dynamodb:table:ReadCapacityUnits",
        starting: 12,
        first: (1, 4),
        update: None,
        expected: 4,
        expected_activities: &["Setting read capacity units to 4."],
    },
    WindowRow {
        case: "a global secondary index's capacity is corrected on its own",
        owner: Owner::DynamoDbIndex,
        namespace: "dynamodb",
        resource_id: "table/catalog/index/by-status",
        dimension: "dynamodb:index:ReadCapacityUnits",
        starting: 1,
        first: (3, 9),
        update: None,
        expected: 3,
        expected_activities: &["Setting read capacity units to 3."],
    },
    WindowRow {
        case: "an ECS service's desired count is raised to the floor",
        owner: Owner::EcsService,
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        starting: 1,
        first: (4, 20),
        update: None,
        expected: 4,
        expected_activities: &["Setting desired count to 4."],
    },
    WindowRow {
        case: "an ECS service's desired count is cut to the ceiling by an update",
        owner: Owner::EcsService,
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        starting: 6,
        first: (0, 20),
        update: Some(|| json!({"MaxCapacity": 2})),
        expected: 2,
        expected_activities: &["Setting desired count to 2."],
    },
    WindowRow {
        case: "two corrections of one resource are two activities, newest first",
        owner: Owner::DynamoDbTable,
        namespace: "dynamodb",
        resource_id: "table/catalog",
        dimension: "dynamodb:table:ReadCapacityUnits",
        starting: 1,
        first: (2, 10),
        update: Some(|| json!({"MinCapacity": 7})),
        expected: 7,
        expected_activities: &[
            "Setting read capacity units to 7.",
            "Setting read capacity units to 2.",
        ],
    },
    WindowRow {
        case: "a capacity already inside the window is left alone",
        owner: Owner::DynamoDbTable,
        namespace: "dynamodb",
        resource_id: "table/catalog",
        dimension: "dynamodb:table:ReadCapacityUnits",
        starting: 6,
        first: (1, 10),
        update: Some(|| json!({"MinCapacity": 2, "MaxCapacity": 20})),
        expected: 6,
        expected_activities: &[],
    },
    WindowRow {
        case: "an ECS desired count already inside the window is left alone",
        owner: Owner::EcsService,
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        starting: 3,
        first: (1, 10),
        update: Some(|| json!({"MaxCapacity": 12})),
        expected: 3,
        expected_activities: &[],
    },
];

/// The three identity members a row's requests carry.
fn identity(row: &WindowRow) -> Value {
    json!({
        "ServiceNamespace": row.namespace,
        "ResourceId": row.resource_id,
        "ScalableDimension": row.dimension,
    })
}

/// `base` with every member of `patch` merged in.
fn merged(base: Value, patch: Value) -> Value {
    let mut merged = base;
    for (member, value) in patch.as_object().expect("an object patch") {
        merged[member] = value.clone();
    }
    merged
}

/// Create the resource a row scales at its starting capacity.
async fn create_resource(emulator: &Emulator, row: &WindowRow) {
    match row.owner {
        Owner::DynamoDbTable => {
            emulator
                .create_provisioned_table("catalog", row.starting, row.starting)
                .await;
        }
        Owner::DynamoDbIndex => {
            emulator
                .create_table_with_index("catalog", "by-status", "status", row.starting, row.starting)
                .await;
        }
        Owner::EcsService => {
            emulator
                .given_ecs_service("prod-cluster", "web-app", row.starting)
                .await;
        }
    }
}

/// The capacity the service that owns a row's resource reports for the property its
/// dimension names.
async fn resource_capacity(emulator: &Emulator, row: &WindowRow) -> i64 {
    let case = row.case;
    match row.owner {
        Owner::DynamoDbTable | Owner::DynamoDbIndex => {
            let (status, body) = emulator
                .call_dynamodb("DescribeTable", json!({"TableName": "catalog"}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeTable to answer 200, got {status} {body:?}"
            );
            let throughput = match row.owner {
                Owner::DynamoDbIndex => {
                    let indexes = body["Table"]["GlobalSecondaryIndexes"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let index = indexes
                        .iter()
                        .find(|index| index["IndexName"] == json!("by-status"))
                        .cloned()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: expected the index to be described, got {body:?}")
                        });
                    index["ProvisionedThroughput"].clone()
                }
                _ => body["Table"]["ProvisionedThroughput"].clone(),
            };
            let member = if row.dimension.ends_with("ReadCapacityUnits") {
                "ReadCapacityUnits"
            } else {
                "WriteCapacityUnits"
            };
            throughput[member].as_i64().unwrap_or_else(|| {
                panic!("case {case:?}: expected a numeric {member}, got {throughput:?}")
            })
        }
        Owner::EcsService => emulator.ecs_desired_count("prod-cluster", "web-app").await,
    }
}

/// The `Description` of every activity the log reports for a row's target, newest first.
async fn activity_descriptions(emulator: &Emulator, row: &WindowRow) -> Vec<String> {
    let case = row.case;
    let (status, body) = emulator
        .call(
            "DescribeScalingActivities",
            merged(identity(row), json!({"IncludeNotScaledActivities": true})),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingActivities to answer 200, got {status} {body:?}"
    );
    body["ScalingActivities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ScalingActivities list, got {body:?}"))
        .iter()
        .map(|activity| activity["Description"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test]
async fn a_registration_places_the_resources_own_capacity_inside_the_window() {
    for row in WINDOW_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        create_resource(&emulator, row).await;

        let (min, max) = row.first;
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                merged(
                    identity(row),
                    json!({"MinCapacity": min, "MaxCapacity": max}),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the registration to answer 200, got {status} {body:?}"
        );
        if let Some(update) = row.update {
            let (status, body) = emulator
                .call("RegisterScalableTarget", merged(identity(row), update()))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the update to answer 200, got {status} {body:?}"
            );
        }

        // The correction reaches the service that OWNS the resource: that is where a client
        // reads the capacity, and the registration is not bookkeeping this service keeps to
        // itself.
        sleep(FULFILMENT_WAIT);
        let capacity = resource_capacity(&emulator, row).await;
        assert_eq!(
            capacity, row.expected,
            "case {case:?}: expected the resource to hold {} after the window was enforced, \
             got {capacity}",
            row.expected
        );

        // And every movement the log claims is one the resource shows — no more, no fewer.
        let listed = activity_descriptions(&emulator, row).await;
        assert_eq!(
            listed, row.expected_activities,
            "case {case:?}: expected the log to report {:?}, got {listed:?}",
            row.expected_activities
        );
    }
}

/// A registration whose resource the owning service does not hold: the case, the resource
/// that IS created first, and the registration that names another.
struct MissingResourceRow {
    case: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    /// Whether the cluster (or table) the row's resource id names is created at all.
    create_sibling: bool,
}

/// The catalog: a resource that was never created, and one whose container exists while the
/// resource itself does not. Both are a target registered "against nothing".
const MISSING_RESOURCE_CATALOG: &[MissingResourceRow] = &[
    MissingResourceRow {
        case: "an ECS service in a cluster that holds none",
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        create_sibling: false,
    },
    MissingResourceRow {
        case: "an ECS service the cluster does not run",
        namespace: "ecs",
        resource_id: "service/prod-cluster/batch-worker",
        dimension: "ecs:service:DesiredCount",
        create_sibling: true,
    },
    MissingResourceRow {
        case: "a DynamoDB table that was never created",
        namespace: "dynamodb",
        resource_id: "table/absent",
        dimension: "dynamodb:table:ReadCapacityUnits",
        create_sibling: false,
    },
];

#[tokio::test]
async fn a_target_on_a_resource_the_owning_service_does_not_hold_is_refused() {
    for row in MISSING_RESOURCE_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        if row.create_sibling {
            emulator
                .given_ecs_service("prod-cluster", "web-app", 1)
                .await;
        }

        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({
                    "ServiceNamespace": row.namespace,
                    "ResourceId": row.resource_id,
                    "ScalableDimension": row.dimension,
                    "MinCapacity": 1,
                    "MaxCapacity": 10,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );

        // A refused registration persists nothing: there is no phantom target to find.
        let (status, described) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": row.namespace}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {described:?}"
        );
        assert_eq!(
            described["ScalableTargets"],
            json!([]),
            "case {case:?}: expected the refusal to persist nothing, got {described:?}"
        );
    }
}

/// One suspended-state case: what the registrations ask for, and the three flags a report
/// must then carry.
struct SuspensionRow {
    case: &'static str,
    /// The `SuspendedState` members each registration names, in order.
    requests: &'static [&'static str],
    /// The three flags, in the order (in, out, scheduled).
    expected: (bool, bool, bool),
}

/// The catalog. Every row reads the same three members back, because the shape declares
/// three and a flag nothing has set is not suspended: the rows differ in how many
/// suspensions were asked for, in which ones, and in whether one was later resumed.
const SUSPENSION_CATALOG: &[SuspensionRow] = &[
    SuspensionRow {
        case: "a registration that says nothing about suspensions",
        requests: &[],
        expected: (false, false, false),
    },
    SuspensionRow {
        case: "scale-in suspended",
        requests: &[r#"{"DynamicScalingInSuspended": true}"#],
        expected: (true, false, false),
    },
    SuspensionRow {
        case: "scale-out suspended",
        requests: &[r#"{"DynamicScalingOutSuspended": true}"#],
        expected: (false, true, false),
    },
    SuspensionRow {
        case: "scheduled scaling suspended",
        requests: &[r#"{"ScheduledScalingSuspended": true}"#],
        expected: (false, false, true),
    },
    SuspensionRow {
        case: "two suspensions, one per registration",
        requests: &[
            r#"{"DynamicScalingInSuspended": true}"#,
            r#"{"ScheduledScalingSuspended": true}"#,
        ],
        expected: (true, false, true),
    },
    SuspensionRow {
        case: "all three at once",
        requests: &[
            r#"{"DynamicScalingInSuspended": true, "DynamicScalingOutSuspended": true, "ScheduledScalingSuspended": true}"#,
        ],
        expected: (true, true, true),
    },
    SuspensionRow {
        case: "a suspension that was resumed again",
        requests: &[
            r#"{"DynamicScalingOutSuspended": true}"#,
            r#"{"DynamicScalingOutSuspended": false}"#,
        ],
        expected: (false, false, false),
    },
];

#[tokio::test]
async fn a_described_target_carries_every_suspended_state_flag() {
    for row in SUSPENSION_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = emulator
            .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
            .await;

        for suspension in row.requests {
            let suspension: Value =
                serde_json::from_str(suspension).expect("the row's SuspendedState parses");
            let (status, body) = emulator
                .call(
                    "RegisterScalableTarget",
                    merged(target.clone(), json!({"SuspendedState": suspension})),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the registration to answer 200, got {status} {body:?}"
            );
        }

        let (status, body) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": "dynamodb"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {body:?}"
        );
        let (expected_in, expected_out, expected_scheduled) = row.expected;
        let expected = json!({
            "DynamicScalingInSuspended": expected_in,
            "DynamicScalingOutSuspended": expected_out,
            "ScheduledScalingSuspended": expected_scheduled,
        });
        let described = body["ScalableTargets"][0]["SuspendedState"].clone();
        assert_eq!(
            described, expected,
            "case {case:?}: expected SuspendedState {expected:?}, got {described:?}"
        );
    }
}
