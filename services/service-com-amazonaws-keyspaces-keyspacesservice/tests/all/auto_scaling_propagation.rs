// SPDX-License-Identifier: CC0-1.0

//! Amazon Keyspaces auto scaling as the Application Auto Scaling resources it IS, driven
//! end to end over both services' own wire protocols on ONE shared engine.
//!
//! The rule under test, exercised over a SET of inputs rather than one example:
//!
//! Configuring a table's auto scaling settings through Amazon Keyspaces registers, for
//! each capacity it configures, that capacity's Application Auto Scaling scalable target
//! (`ServiceNamespace` `cassandra`, `ResourceId` `keyspace/<keyspaceName>/table/<tableName>`,
//! the capacity's `ScalableDimension`, `MinCapacity`/`MaxCapacity` from
//! `minimumUnits`/`maximumUnits`) and the target-tracking scaling policy built from its
//! `scalingPolicy.targetTrackingScalingPolicyConfiguration`; turning auto scaling off for
//! a capacity, or deleting the table, retires them; and the settings Amazon Keyspaces
//! reports are the ones Application Auto Scaling holds, whichever service wrote them.
//!
//! The rows differ along which capacity is configured, whether auto scaling is on, the
//! unit bounds, the target value, the cooldowns and `disableScaleIn`, and the names of the
//! keyspace and table the resource id is built from.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// Amazon Keyspaces and Application Auto Scaling over ONE engine — what `core/router`
/// assembles — so a Keyspaces write and an Application Auto Scaling read see one store.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn start() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// Call an Amazon Keyspaces operation.
    async fn keyspaces(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch(
            "cassandra",
            "application/x-amz-json-1.0",
            &format!("KeyspacesService.{operation}"),
            body,
        )
        .await
    }

    /// Call an Application Auto Scaling operation.
    async fn auto_scaling(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch(
            "application-autoscaling",
            "application/x-amz-json-1.1",
            &format!("AnyScaleFrontendService.{operation}"),
            body,
        )
        .await
    }

    async fn dispatch(
        &self,
        signing_name: &str,
        content_type: &str,
        target: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", content_type)
            .header("x-amz-target", target)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}\
                     /aws4_request, SignedHeaders=host;x-amz-target, Signature=abc123"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the test request body serializes"),
            ))
            .expect("a well-formed test request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Every `cassandra` scalable target Application Auto Scaling holds, ordered by
    /// resource id and dimension.
    async fn cassandra_targets(&self) -> Vec<Value> {
        let (status, body) = self
            .auto_scaling(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": "cassandra"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "expected DescribeScalableTargets to answer 200, got {status} {body}"
        );
        let mut targets = body["ScalableTargets"].as_array().cloned().unwrap_or_default();
        targets.sort_by_key(|target| {
            (
                target["ResourceId"].as_str().unwrap_or_default().to_string(),
                target["ScalableDimension"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        });
        targets
    }

    /// Every `cassandra` scaling policy Application Auto Scaling holds, ordered by the
    /// dimension it steers.
    async fn cassandra_policies(&self) -> Vec<Value> {
        let (status, body) = self
            .auto_scaling(
                "DescribeScalingPolicies",
                json!({"ServiceNamespace": "cassandra"}),
            )
            .await;
        assert_eq!(
            status, 200,
            "expected DescribeScalingPolicies to answer 200, got {status} {body}"
        );
        let mut policies = body["ScalingPolicies"].as_array().cloned().unwrap_or_default();
        policies.sort_by_key(|policy| {
            (
                policy["ResourceId"].as_str().unwrap_or_default().to_string(),
                policy["ScalableDimension"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        });
        policies
    }

    /// Create `keyspace`, then a PROVISIONED table in it carrying `specification` (when
    /// one is given), and assert both calls succeeded.
    async fn given_provisioned_table(
        &self,
        case: &str,
        keyspace: &str,
        table: &str,
        specification: Option<&Value>,
    ) {
        let (status, body) = self
            .keyspaces("CreateKeyspace", json!({"keyspaceName": keyspace}))
            .await;
        assert!(
            status == 200 || status == 409,
            "case {case:?}: expected CreateKeyspace to answer 200 or 409, got {status} {body}"
        );
        let mut request = json!({
            "keyspaceName": keyspace,
            "tableName": table,
            "schemaDefinition": {
                "allColumns": [{"name": "pk", "type": "text"}],
                "partitionKeys": [{"name": "pk"}],
            },
            "capacitySpecification": {
                "throughputMode": "PROVISIONED",
                "readCapacityUnits": 5,
                "writeCapacityUnits": 5,
            },
        });
        if let Some(specification) = specification {
            request["autoScalingSpecification"] = specification.clone();
        }
        let (status, body) = self.keyspaces("CreateTable", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateTable to answer 200, got {status} {body}"
        );
    }
}

/// The settings that turn a capacity's auto scaling ON, as a request carries them.
fn enabled(minimum: i64, maximum: i64, target: f64) -> Value {
    json!({
        "autoScalingDisabled": false,
        "minimumUnits": minimum,
        "maximumUnits": maximum,
        "scalingPolicy": {
            "targetTrackingScalingPolicyConfiguration": {"targetValue": target},
        },
    })
}

/// The `ResourceId` Application Auto Scaling documents for the `cassandra` namespace's
/// table resource type: `keyspace/<keyspaceName>/table/<tableName>`.
fn resource_id(keyspace: &str, table: &str) -> String {
    format!("keyspace/{keyspace}/table/{table}")
}

/// Compare one member of a JSON body, by pointer, tolerating the integer and the double
/// spelling of the same number.
fn assert_member(case: &str, body: &Value, pointer: &str, expected: &Value) {
    let actual = body.pointer(pointer).unwrap_or(&Value::Null);
    let equal = match (actual.as_f64(), expected.as_f64()) {
        (Some(actual), Some(expected)) => (actual - expected).abs() < 1e-9,
        _ => actual == expected,
    };
    assert!(
        equal,
        "case {case:?}: {pointer} expected {expected}, got {actual} in {body}"
    );
}

/// The window a scalable target must be in: its dimension, minimum, maximum and the
/// target value of the policy that steers it.
type ExpectedWindow = (&'static str, i64, i64, f64);

/// One row: the specification a table is created with, and the capacities it must leave a
/// scalable target registered for.
struct Case {
    name: &'static str,
    keyspace: &'static str,
    table: &'static str,
    specification: Value,
    /// Per expected target: the dimension, the minimum, the maximum, the target value,
    /// the scale-in cooldown, the scale-out cooldown and `DisableScaleIn`.
    expected: Vec<(&'static str, i64, i64, f64, i64, i64, bool)>,
}

/// RULE: a table's auto scaling settings become the scalable target and the
/// target-tracking policy of each capacity they configure, and nothing for a capacity
/// they turn auto scaling off for — reported back by Application Auto Scaling's own
/// `DescribeScalableTargets` / `DescribeScalingPolicies` and by Amazon Keyspaces'
/// `GetTableAutoScalingSettings` alike.
#[tokio::test]
async fn create_table_registers_a_scalable_target_per_configured_capacity() {
    let read_dimension = keyspaces::TableCapacity::Read.scalable_dimension();
    let write_dimension = keyspaces::TableCapacity::Write.scalable_dimension();
    let cases = vec![
        Case {
            name: "read_capacity_only",
            keyspace: "propagation_ks",
            table: "read_only_tbl",
            specification: json!({"readCapacityAutoScaling": enabled(5, 50, 60.0)}),
            expected: vec![(read_dimension, 5, 50, 60.0, 0, 0, false)],
        },
        Case {
            name: "write_capacity_only",
            keyspace: "propagation_ks",
            table: "write_only_tbl",
            specification: json!({"writeCapacityAutoScaling": enabled(2, 9, 20.0)}),
            expected: vec![(write_dimension, 2, 9, 20.0, 0, 0, false)],
        },
        Case {
            name: "both_capacities",
            keyspace: "propagation_ks",
            table: "both_tbl",
            specification: json!({
                "readCapacityAutoScaling": enabled(1, 40000, 90.0),
                "writeCapacityAutoScaling": enabled(7, 7, 20.0),
            }),
            expected: vec![
                (read_dimension, 1, 40000, 90.0, 0, 0, false),
                (write_dimension, 7, 7, 20.0, 0, 0, false),
            ],
        },
        Case {
            name: "cooldowns_and_scale_in_disabled",
            keyspace: "other_propagation_ks",
            table: "cooldown_tbl",
            specification: json!({
                "readCapacityAutoScaling": {
                    "autoScalingDisabled": false,
                    "minimumUnits": 10,
                    "maximumUnits": 100,
                    "scalingPolicy": {
                        "targetTrackingScalingPolicyConfiguration": {
                            "targetValue": 75.5,
                            "disableScaleIn": true,
                            "scaleInCooldown": 120,
                            "scaleOutCooldown": 30,
                        },
                    },
                },
            }),
            expected: vec![(read_dimension, 10, 100, 75.5, 120, 30, true)],
        },
        Case {
            // "This optional parameter enables auto scaling for the table if set to
            // false": turned off for both capacities, so neither is scaled by anything
            // and neither has a scalable target.
            name: "explicitly_disabled",
            keyspace: "other_propagation_ks",
            table: "disabled_tbl",
            specification: json!({
                "readCapacityAutoScaling": {"autoScalingDisabled": true},
                "writeCapacityAutoScaling": {"autoScalingDisabled": true},
            }),
            expected: Vec::new(),
        },
        Case {
            name: "no_auto_scaling_at_all",
            keyspace: "other_propagation_ks",
            table: "unscaled_tbl",
            specification: Value::Null,
            expected: Vec::new(),
        },
    ];

    for case in &cases {
        // One emulator per row, so what `DescribeScalableTargets` answers is exactly what
        // this row's table left behind.
        let emulator = Emulator::start();
        let specification = (!case.specification.is_null()).then(|| case.specification.clone());
        emulator
            .given_provisioned_table(case.name, case.keyspace, case.table, specification.as_ref())
            .await;

        let targets = emulator.cassandra_targets().await;
        assert_eq!(
            targets.len(),
            case.expected.len(),
            "case {:?}: expected {} cassandra scalable target(s), got {targets:?}",
            case.name,
            case.expected.len()
        );
        let policies = emulator.cassandra_policies().await;
        assert_eq!(
            policies.len(),
            case.expected.len(),
            "case {:?}: expected {} cassandra scaling policy/policies, got {policies:?}",
            case.name,
            case.expected.len()
        );

        for (index, (dimension, minimum, maximum, target_value, scale_in, scale_out, no_scale_in)) in
            case.expected.iter().enumerate()
        {
            let target = &targets[index];
            let name = format!("{} {dimension}", case.name);
            assert_member(&name, target, "/ServiceNamespace", &json!("cassandra"));
            assert_member(
                &name,
                target,
                "/ResourceId",
                &json!(resource_id(case.keyspace, case.table)),
            );
            assert_member(&name, target, "/ScalableDimension", &json!(dimension));
            assert_member(&name, target, "/MinCapacity", &json!(minimum));
            assert_member(&name, target, "/MaxCapacity", &json!(maximum));
            // A scalable target is a resource: it carries its own ARN and the
            // service-linked role Application Auto Scaling scales `cassandra` through.
            let role = target["RoleARN"].as_str().unwrap_or_default();
            assert!(
                role.contains("Cassandra"),
                "case {name:?}: expected the cassandra service-linked role, got {role:?}"
            );
            assert!(
                target["ScalableTargetARN"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(":scalable-target/"),
                "case {name:?}: expected a scalable-target ARN, got {target}"
            );

            let policy = &policies[index];
            assert_member(&name, policy, "/ScalableDimension", &json!(dimension));
            assert_member(&name, policy, "/PolicyType", &json!("TargetTrackingScaling"));
            let configuration = "/TargetTrackingScalingPolicyConfiguration";
            assert_member(
                &name,
                policy,
                &format!("{configuration}/TargetValue"),
                &json!(target_value),
            );
            assert_member(
                &name,
                policy,
                &format!("{configuration}/ScaleInCooldown"),
                &json!(scale_in),
            );
            assert_member(
                &name,
                policy,
                &format!("{configuration}/ScaleOutCooldown"),
                &json!(scale_out),
            );
            assert_member(
                &name,
                policy,
                &format!("{configuration}/DisableScaleIn"),
                &json!(no_scale_in),
            );
            // The policy steers on the Amazon Keyspaces utilisation metric Application
            // Auto Scaling publishes for this very dimension.
            let metric = if *dimension == keyspaces::TableCapacity::Read.scalable_dimension() {
                keyspaces::TableCapacity::Read.predefined_metric()
            } else {
                keyspaces::TableCapacity::Write.predefined_metric()
            };
            assert_member(
                &name,
                policy,
                &format!("{configuration}/PredefinedMetricSpecification/PredefinedMetricType"),
                &json!(metric),
            );
            // A target-tracking policy IS its alarms, so the policy owns the ones that
            // drive it.
            let alarms = policy["Alarms"].as_array().cloned().unwrap_or_default();
            assert!(
                !alarms.is_empty(),
                "case {name:?}: expected the policy to own its CloudWatch alarms, got {policy}"
            );
        }

        // And Amazon Keyspaces reports those same settings back, because they are the
        // settings it reads out of Application Auto Scaling.
        let (status, body) = emulator
            .keyspaces(
                "GetTableAutoScalingSettings",
                json!({"keyspaceName": case.keyspace, "tableName": case.table}),
            )
            .await;
        if case.expected.is_empty() && case.specification.is_null() {
            // A table auto scaling was never configured for has no settings at all.
            assert_eq!(
                status, 404,
                "case {:?}: expected the modelled not-found, got {status} {body}",
                case.name
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {:?}: expected GetTableAutoScalingSettings to answer 200, got {status} {body}",
            case.name
        );
        for (dimension, minimum, maximum, target_value, scale_in, scale_out, no_scale_in) in
            &case.expected
        {
            let member = if *dimension == keyspaces::TableCapacity::Read.scalable_dimension() {
                "readCapacityAutoScaling"
            } else {
                "writeCapacityAutoScaling"
            };
            let base = format!("/autoScalingSpecification/{member}");
            let name = format!("{} {dimension} reported back", case.name);
            assert_member(
                &name,
                &body,
                &format!("{base}/autoScalingDisabled"),
                &json!(false),
            );
            assert_member(&name, &body, &format!("{base}/minimumUnits"), &json!(minimum));
            assert_member(&name, &body, &format!("{base}/maximumUnits"), &json!(maximum));
            let policy = format!(
                "{base}/scalingPolicy/targetTrackingScalingPolicyConfiguration"
            );
            assert_member(&name, &body, &format!("{policy}/targetValue"), &json!(target_value));
            assert_member(
                &name,
                &body,
                &format!("{policy}/scaleInCooldown"),
                &json!(scale_in),
            );
            assert_member(
                &name,
                &body,
                &format!("{policy}/scaleOutCooldown"),
                &json!(scale_out),
            );
            assert_member(
                &name,
                &body,
                &format!("{policy}/disableScaleIn"),
                &json!(no_scale_in),
            );
        }
        // A capacity auto scaling is turned off for has no target, and Amazon Keyspaces
        // reports that setting as the caller gave it.
        if case.name == "explicitly_disabled" {
            for member in ["readCapacityAutoScaling", "writeCapacityAutoScaling"] {
                assert_member(
                    case.name,
                    &body,
                    &format!("/autoScalingSpecification/{member}/autoScalingDisabled"),
                    &json!(true),
                );
            }
        }
    }
}

/// RULE: an update that changes a capacity's auto scaling settings updates that
/// capacity's scalable target and policy, independently of the other capacity's; one that
/// turns auto scaling off for a capacity deregisters its target; and one that takes the
/// table out of provisioned capacity mode deregisters both.
#[tokio::test]
async fn update_table_re_registers_and_deregisters_the_capacity_it_configures() {
    let emulator = Emulator::start();
    let (keyspace, table) = ("update_propagation_ks", "scaled_tbl");
    emulator
        .given_provisioned_table(
            "both capacities configured",
            keyspace,
            table,
            Some(&json!({
                "readCapacityAutoScaling": enabled(5, 50, 60.0),
                "writeCapacityAutoScaling": enabled(5, 50, 60.0),
            })),
        )
        .await;
    tokio::time::sleep(keyspaces::TRANSIENT_STATUS_SPAN + std::time::Duration::from_millis(250))
        .await;

    // Each row: what the update configures, and the window every cassandra target must
    // then be in, in resource-id/dimension order.
    struct Row {
        case: &'static str,
        specification: Value,
        expected: Vec<ExpectedWindow>,
    }
    let rows: Vec<Row> = vec![
        Row {
            case: "widen the read window",
            specification: json!({"readCapacityAutoScaling": enabled(10, 200, 70.0)}),
            expected: vec![
                (
                    keyspaces::TableCapacity::Read.scalable_dimension(),
                    10,
                    200,
                    70.0,
                ),
                (
                    keyspaces::TableCapacity::Write.scalable_dimension(),
                    5,
                    50,
                    60.0,
                ),
            ],
        },
        Row {
            case: "narrow the write window",
            specification: json!({"writeCapacityAutoScaling": enabled(1, 3, 20.0)}),
            expected: vec![
                (
                    keyspaces::TableCapacity::Read.scalable_dimension(),
                    10,
                    200,
                    70.0,
                ),
                (
                    keyspaces::TableCapacity::Write.scalable_dimension(),
                    1,
                    3,
                    20.0,
                ),
            ],
        },
        Row {
            case: "turn read capacity auto scaling off",
            specification: json!({"readCapacityAutoScaling": {"autoScalingDisabled": true}}),
            expected: vec![(
                keyspaces::TableCapacity::Write.scalable_dimension(),
                1,
                3,
                20.0,
            )],
        },
        Row {
            case: "turn it back on",
            specification: json!({"readCapacityAutoScaling": enabled(4, 44, 44.0)}),
            expected: vec![
                (
                    keyspaces::TableCapacity::Read.scalable_dimension(),
                    4,
                    44,
                    44.0,
                ),
                (
                    keyspaces::TableCapacity::Write.scalable_dimension(),
                    1,
                    3,
                    20.0,
                ),
            ],
        },
    ];

    for Row {
        case,
        specification,
        expected,
    } in &rows
    {
        let (status, body) = emulator
            .keyspaces(
                "UpdateTable",
                json!({
                    "keyspaceName": keyspace,
                    "tableName": table,
                    "autoScalingSpecification": specification,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateTable to answer 200, got {status} {body}"
        );
        let targets = emulator.cassandra_targets().await;
        assert_eq!(
            targets.len(),
            expected.len(),
            "case {case:?}: expected {} target(s), got {targets:?}",
            expected.len()
        );
        for (index, (dimension, minimum, maximum, target_value)) in expected.iter().enumerate() {
            let target = &targets[index];
            assert_member(case, target, "/ScalableDimension", &json!(dimension));
            assert_member(case, target, "/MinCapacity", &json!(minimum));
            assert_member(case, target, "/MaxCapacity", &json!(maximum));
            let policy = emulator
                .cassandra_policies()
                .await
                .into_iter()
                .find(|policy| policy["ScalableDimension"] == json!(dimension))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no scaling policy steers {dimension}")
                });
            assert_member(
                case,
                &policy,
                "/TargetTrackingScalingPolicyConfiguration/TargetValue",
                &json!(target_value),
            );
        }
        // The target that was deregistered takes its policy with it.
        let policies = emulator.cassandra_policies().await;
        assert_eq!(
            policies.len(),
            expected.len(),
            "case {case:?}: expected {} policy/policies, got {policies:?}",
            expected.len()
        );
        tokio::time::sleep(keyspaces::TRANSIENT_STATUS_SPAN + std::time::Duration::from_millis(250))
            .await;
    }

    // And a table moved OUT of provisioned capacity mode has no provisioned throughput
    // for a policy to move, so both capacities are deregistered.
    let (status, body) = emulator
        .keyspaces(
            "UpdateTable",
            json!({
                "keyspaceName": keyspace,
                "tableName": table,
                "capacitySpecification": {"throughputMode": "PAY_PER_REQUEST"},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"move to PAY_PER_REQUEST\": expected UpdateTable to answer 200, got {status} {body}"
    );
    let targets = emulator.cassandra_targets().await;
    assert!(
        targets.is_empty(),
        "case \"move to PAY_PER_REQUEST\": expected no scalable target to survive, got {targets:?}"
    );
    let policies = emulator.cassandra_policies().await;
    assert!(
        policies.is_empty(),
        "case \"move to PAY_PER_REQUEST\": expected no scaling policy to survive, got {policies:?}"
    );
}

/// RULE: deleting the resource the targets scale retires them — whether the table is
/// deleted on its own or with the keyspace that holds it.
#[tokio::test]
async fn deleting_the_table_or_its_keyspace_deregisters_its_scalable_targets() {
    for (case, operation, request) in [
        (
            "DeleteTable",
            "DeleteTable",
            json!({"keyspaceName": "deletion_ks", "tableName": "scaled_tbl"}),
        ),
        (
            "DeleteKeyspace",
            "DeleteKeyspace",
            json!({"keyspaceName": "deletion_ks"}),
        ),
    ] {
        let emulator = Emulator::start();
        emulator
            .given_provisioned_table(
                case,
                "deletion_ks",
                "scaled_tbl",
                Some(&json!({
                    "readCapacityAutoScaling": enabled(5, 50, 60.0),
                    "writeCapacityAutoScaling": enabled(5, 50, 60.0),
                })),
            )
            .await;
        assert_eq!(
            emulator.cassandra_targets().await.len(),
            2,
            "case {case:?}: both capacities should be scaled before the delete"
        );
        tokio::time::sleep(keyspaces::TRANSIENT_STATUS_SPAN + std::time::Duration::from_millis(250))
            .await;

        let (status, body) = emulator.keyspaces(operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {body}"
        );
        let targets = emulator.cassandra_targets().await;
        assert!(
            targets.is_empty(),
            "case {case:?}: expected every scalable target to be deregistered, got {targets:?}"
        );
        let policies = emulator.cassandra_policies().await;
        assert!(
            policies.is_empty(),
            "case {case:?}: expected every scaling policy to go with its target, got {policies:?}"
        );
    }
}

/// RULE, read from the other side: the settings Amazon Keyspaces reports are the ones
/// Application Auto Scaling holds, so a target and a policy a caller registers THERE for
/// a Keyspaces table are what `GetTableAutoScalingSettings` reports — and deregistering
/// one through Application Auto Scaling removes those settings.
#[tokio::test]
async fn settings_registered_through_application_auto_scaling_are_reported_by_keyspaces() {
    let emulator = Emulator::start();
    let (keyspace, table) = ("direct_ks", "direct_tbl");
    emulator
        .given_provisioned_table("a table with no auto scaling", keyspace, table, None)
        .await;

    // Before anything is registered, the table has no auto scaling settings.
    let (status, body) = emulator
        .keyspaces(
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"unscaled\": expected the modelled not-found, got {status} {body}"
    );

    // Registered directly through Application Auto Scaling, for each capacity in turn.
    for (case, capacity, minimum, maximum, target_value) in [
        ("read registered directly", keyspaces::TableCapacity::Read, 3, 30, 55.0),
        (
            "write registered directly",
            keyspaces::TableCapacity::Write,
            8,
            80,
            35.0,
        ),
    ] {
        let identity = json!({
            "ServiceNamespace": "cassandra",
            "ResourceId": resource_id(keyspace, table),
            "ScalableDimension": capacity.scalable_dimension(),
        });
        let mut registration = identity.clone();
        registration["MinCapacity"] = json!(minimum);
        registration["MaxCapacity"] = json!(maximum);
        let (status, body) = emulator
            .auto_scaling("RegisterScalableTarget", registration)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body}"
        );
        let mut put = identity.clone();
        put["PolicyName"] = json!(format!("{}-by-hand", capacity.scalable_dimension()));
        put["PolicyType"] = json!("TargetTrackingScaling");
        put["TargetTrackingScalingPolicyConfiguration"] = json!({
            "TargetValue": target_value,
            "PredefinedMetricSpecification": {
                "PredefinedMetricType": capacity.predefined_metric(),
            },
        });
        let (status, body) = emulator.auto_scaling("PutScalingPolicy", put).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body}"
        );

        let (status, body) = emulator
            .keyspaces(
                "GetTableAutoScalingSettings",
                json!({"keyspaceName": keyspace, "tableName": table}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetTableAutoScalingSettings to answer 200, got {status} {body}"
        );
        let member = match capacity {
            keyspaces::TableCapacity::Read => "readCapacityAutoScaling",
            keyspaces::TableCapacity::Write => "writeCapacityAutoScaling",
        };
        let base = format!("/autoScalingSpecification/{member}");
        assert_member(case, &body, &format!("{base}/minimumUnits"), &json!(minimum));
        assert_member(case, &body, &format!("{base}/maximumUnits"), &json!(maximum));
        assert_member(
            case,
            &body,
            &format!("{base}/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue"),
            &json!(target_value),
        );
    }

    // Deregistered there, the settings are gone from the Keyspaces report too: the read
    // capacity's go first, leaving only the write capacity's.
    let (status, body) = emulator
        .auto_scaling(
            "DeregisterScalableTarget",
            json!({
                "ServiceNamespace": "cassandra",
                "ResourceId": resource_id(keyspace, table),
                "ScalableDimension": keyspaces::TableCapacity::Read.scalable_dimension(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"deregistered read\": expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .keyspaces(
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"deregistered read\": expected 200, got {status} {body}"
    );
    assert_member(
        "deregistered read",
        &body,
        "/autoScalingSpecification/readCapacityAutoScaling",
        &Value::Null,
    );
    assert_member(
        "deregistered read",
        &body,
        "/autoScalingSpecification/writeCapacityAutoScaling/minimumUnits",
        &json!(8),
    );

    // With the last target deregistered the table has no auto scaling settings again.
    let (status, body) = emulator
        .auto_scaling(
            "DeregisterScalableTarget",
            json!({
                "ServiceNamespace": "cassandra",
                "ResourceId": resource_id(keyspace, table),
                "ScalableDimension": keyspaces::TableCapacity::Write.scalable_dimension(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"deregistered write\": expected 200, got {status} {body}"
    );
    let (status, body) = emulator
        .keyspaces(
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"deregistered both\": expected the modelled not-found, got {status} {body}"
    );
}
