// SPDX-License-Identifier: CC0-1.0

//! `RestoreTable` reproducing the source table's auto scaling settings AS THEY STAND when
//! the restore is called, driven end to end over both services' own wire protocols on ONE
//! shared engine — Amazon Keyspaces and Application Auto Scaling, as `core/router`
//! assembles them.
//!
//! The rule under test, exercised over a SET of inputs rather than one example: a table's
//! auto scaling settings ARE the scalable targets Application Auto Scaling holds for its
//! capacities and the target-tracking policies that steer them, so the settings a restore
//! reproduces for the restored table are the ones that store holds for the source table at
//! the time of the call — whichever service registered, re-tuned or deregistered them —
//! and a capacity with no scalable target carries nothing over.
//!
//! The rows differ along which capacity was written straight through Application Auto
//! Scaling, whether Amazon Keyspaces had configured that capacity first, the unit bounds
//! and target values, whether the live target was re-tuned or deregistered, the name the
//! policy was written under, and the keyspace and table names.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use super::auto_scaling_propagation::{enabled, expected_policy_name, resource_id, Emulator};
use super::keyspace_table_lifecycle::settle_wait;

/// What one capacity's settings are expected to be: the unit window and the target value
/// of the policy that steers it, or `None` for a capacity that has no settings at all.
type ExpectedSettings = Option<(i64, i64, f64)>;

/// One row: a source table, the settings written to it through either service, and what
/// both it and the table restored from it must then report.
struct Case {
    /// The case name.
    name: &'static str,
    /// The keyspace both tables live in.
    keyspace: &'static str,
    /// The source table, created in provisioned capacity mode with point-in-time recovery
    /// on, so it has a restorable window.
    source: &'static str,
    /// The table the restore creates.
    restored: &'static str,
    /// The `autoScalingSpecification` the source table is CREATED with, when it has one.
    created_with: Option<Value>,
    /// Written straight to Application Auto Scaling for the source table afterwards:
    /// `(capacity, minimumUnits, maximumUnits, targetValue)` per registration.
    registered_directly: Vec<(keyspaces::TableCapacity, i64, i64, f64)>,
    /// The name those registrations put the policy under: `None` re-tunes the policy
    /// Amazon Keyspaces registered for the table, under the name it gave it, and `Some`
    /// is a name the caller chose itself.
    direct_policy_name: Option<&'static str>,
    /// Capacities whose scalable target is deregistered straight through Application Auto
    /// Scaling afterwards.
    deregistered: Vec<keyspaces::TableCapacity>,
    /// Per capacity, the settings the source table then has — and therefore the ones the
    /// restored table must land with.
    expect: Vec<(keyspaces::TableCapacity, ExpectedSettings)>,
}

/// The rows, so the rule is asserted across the class rather than on one example.
fn cases() -> Vec<Case> {
    use keyspaces::TableCapacity::{Read, Write};
    vec![
        Case {
            // A capacity registered through Application Auto Scaling itself on a table
            // Amazon Keyspaces never configured auto scaling for: there is no stored
            // specification at all, and the live target is the whole of the settings.
            name: "read_registered_directly_on_an_unconfigured_source",
            keyspace: "restore_live_as_ks",
            source: "direct_read_src",
            restored: "direct_read_dst",
            created_with: None,
            registered_directly: vec![(Read, 3, 30, 55.0)],
            direct_policy_name: Some("read-registered-by-hand"),
            deregistered: Vec::new(),
            expect: vec![(Read, Some((3, 30, 55.0))), (Write, None)],
        },
        Case {
            // Both capacities re-tuned through Application Auto Scaling after Amazon
            // Keyspaces configured them: the windows and the target values the restore
            // reproduces are the re-tuned ones, not the ones the create wrote.
            name: "both_capacities_retuned_directly",
            keyspace: "restore_live_as_ks",
            source: "retuned_src",
            restored: "retuned_dst",
            created_with: Some(json!({
                "readCapacityAutoScaling": enabled(5, 50, 70.0),
                "writeCapacityAutoScaling": enabled(5, 50, 70.0),
            })),
            registered_directly: vec![(Read, 7, 70, 25.0), (Write, 2, 20, 80.0)],
            direct_policy_name: None,
            deregistered: Vec::new(),
            expect: vec![(Read, Some((7, 70, 25.0))), (Write, Some((2, 20, 80.0)))],
        },
        Case {
            // Both capacities deregistered through Application Auto Scaling: the source
            // table has no auto scaling settings left, so the restored table lands with
            // none either.
            name: "both_capacities_deregistered_directly",
            keyspace: "restore_live_as_ks",
            source: "deregistered_src",
            restored: "deregistered_dst",
            created_with: Some(json!({
                "readCapacityAutoScaling": enabled(6, 60, 45.0),
                "writeCapacityAutoScaling": enabled(6, 60, 45.0),
            })),
            registered_directly: Vec::new(),
            direct_policy_name: None,
            deregistered: vec![Read, Write],
            expect: vec![(Read, None), (Write, None)],
        },
        Case {
            // One capacity deregistered, the other left exactly as Amazon Keyspaces wrote
            // it: the two capacities are scaled independently, so only the one with a
            // live target carries over. A different keyspace name, too.
            name: "write_deregistered_read_left_as_written",
            keyspace: "restore_live_as_other_ks",
            source: "half_src",
            restored: "half_dst",
            created_with: Some(json!({
                "readCapacityAutoScaling": enabled(4, 40, 65.0),
                "writeCapacityAutoScaling": enabled(9, 90, 35.0),
            })),
            registered_directly: Vec::new(),
            direct_policy_name: None,
            deregistered: vec![Write],
            expect: vec![(Read, Some((4, 40, 65.0))), (Write, None)],
        },
    ]
}

/// The member of `autoScalingSpecification` one capacity's settings are reported under.
fn reported_member(capacity: keyspaces::TableCapacity) -> &'static str {
    match capacity {
        keyspaces::TableCapacity::Read => "readCapacityAutoScaling",
        keyspaces::TableCapacity::Write => "writeCapacityAutoScaling",
    }
}

/// Compare one member of a JSON body, by pointer, tolerating the integer and the double
/// spelling of the same number.
fn assert_member(case: &str, label: &str, body: &Value, pointer: &str, expected: &Value) {
    let actual = body.pointer(pointer).unwrap_or(&Value::Null);
    let equal = match (actual.as_f64(), expected.as_f64()) {
        (Some(actual), Some(expected)) => (actual - expected).abs() < 1e-9,
        _ => actual == expected,
    };
    assert!(
        equal,
        "case {case:?} ({label}): {pointer} expected {expected}, got {actual} in {body}"
    );
}

/// Assert that `GetTableAutoScalingSettings` reports exactly `expect` for one table.
async fn assert_reported(
    case: &str,
    label: &str,
    emulator: &Emulator,
    keyspace: &str,
    table: &str,
    expect: &[(keyspaces::TableCapacity, ExpectedSettings)],
) {
    let (status, body) = emulator
        .keyspaces(
            "GetTableAutoScalingSettings",
            json!({"keyspaceName": keyspace, "tableName": table}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?} ({label}): expected GetTableAutoScalingSettings to answer 200, \
         got {status} {body}"
    );
    // The member is OPTIONAL, and a table with no settings at all is reported without it.
    let configured = expect.iter().any(|(_, settings)| settings.is_some());
    assert_eq!(
        body.pointer("/autoScalingSpecification").is_some(),
        configured,
        "case {case:?} ({label}): expected autoScalingSpecification present={configured}, \
         got {body}"
    );
    for (capacity, settings) in expect {
        let base = format!("/autoScalingSpecification/{}", reported_member(*capacity));
        match settings {
            None => assert!(
                body.pointer(&base).is_none(),
                "case {case:?} ({label}): expected no {base}, got {body}"
            ),
            Some((minimum, maximum, target_value)) => {
                assert_member(case, label, &body, &format!("{base}/minimumUnits"), &json!(minimum));
                assert_member(case, label, &body, &format!("{base}/maximumUnits"), &json!(maximum));
                assert_member(
                    case,
                    label,
                    &body,
                    &format!("{base}/autoScalingDisabled"),
                    &json!(false),
                );
                assert_member(
                    case,
                    label,
                    &body,
                    &format!(
                        "{base}/scalingPolicy/targetTrackingScalingPolicyConfiguration/targetValue"
                    ),
                    &json!(target_value),
                );
            }
        }
    }
}

/// Assert what Application Auto Scaling itself holds for one table: a scalable target and
/// a policy of this service's own naming per capacity that has settings, and neither for a
/// capacity that has none.
async fn assert_held_by_auto_scaling(
    case: &str,
    label: &str,
    emulator: &Emulator,
    keyspace: &str,
    table: &str,
    expect: &[(keyspaces::TableCapacity, ExpectedSettings)],
) {
    let identity = json!(resource_id(keyspace, table));
    let targets: Vec<Value> = emulator
        .cassandra_targets()
        .await
        .into_iter()
        .filter(|target| target["ResourceId"] == identity)
        .collect();
    let policies: Vec<Value> = emulator
        .cassandra_policies()
        .await
        .into_iter()
        .filter(|policy| policy["ResourceId"] == identity)
        .collect();
    for (capacity, settings) in expect {
        let dimension = json!(capacity.scalable_dimension());
        let target = targets
            .iter()
            .find(|target| target["ScalableDimension"] == dimension);
        let policy = policies
            .iter()
            .find(|policy| policy["ScalableDimension"] == dimension);
        match settings {
            None => {
                assert!(
                    target.is_none(),
                    "case {case:?} ({label}): expected no scalable target for \
                     {dimension}, got {target:?}"
                );
                assert!(
                    policy.is_none(),
                    "case {case:?} ({label}): expected no scaling policy for \
                     {dimension}, got {policy:?}"
                );
            }
            Some((minimum, maximum, target_value)) => {
                let Some(target) = target else {
                    panic!(
                        "case {case:?} ({label}): expected a scalable target for \
                         {dimension}, got {targets:?}"
                    );
                };
                assert_member(case, label, target, "/MinCapacity", &json!(minimum));
                assert_member(case, label, target, "/MaxCapacity", &json!(maximum));
                let Some(policy) = policy else {
                    panic!(
                        "case {case:?} ({label}): expected a scaling policy for \
                         {dimension}, got {policies:?}"
                    );
                };
                assert_member(
                    case,
                    label,
                    policy,
                    "/PolicyName",
                    &json!(expected_policy_name(keyspace, table)),
                );
                assert_member(
                    case,
                    label,
                    policy,
                    "/TargetTrackingScalingPolicyConfiguration/TargetValue",
                    &json!(target_value),
                );
            }
        }
    }
}

/// Create the case's keyspace and its source table, and wait for the table to settle into
/// `ACTIVE` — the only status a restore may read it in.
async fn given_source_table(case: &Case, emulator: &Emulator) {
    let (status, body) = emulator
        .keyspaces("CreateKeyspace", json!({"keyspaceName": case.keyspace}))
        .await;
    // A keyspace an earlier row already created answers the modelled conflict, which this
    // seed tolerates; what it does not tolerate is any OTHER answer.
    let conflicted = body["__type"]
        .as_str()
        .is_some_and(|wire_type| wire_type.ends_with("ConflictException"));
    assert!(
        status == 200 || conflicted,
        "case {:?}: expected CreateKeyspace to answer 200 or the modelled \
         ConflictException, got {status} {body}",
        case.name
    );
    let mut request = json!({
        "keyspaceName": case.keyspace,
        "tableName": case.source,
        "schemaDefinition": {
            "allColumns": [{"name": "pk", "type": "text"}],
            "partitionKeys": [{"name": "pk"}],
        },
        "capacitySpecification": {
            "throughputMode": "PROVISIONED",
            "readCapacityUnits": 5,
            "writeCapacityUnits": 5,
        },
        // Without point-in-time recovery a table has no restorable window at all.
        "pointInTimeRecovery": {"status": "ENABLED"},
    });
    if let Some(specification) = case.created_with.as_ref() {
        request["autoScalingSpecification"] = specification.clone();
    }
    let (status, body) = emulator.keyspaces("CreateTable", request).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateTable to answer 200, got {status} {body}",
        case.name
    );
    tokio::time::sleep(settle_wait()).await;
}

/// Write the case's Application Auto Scaling registrations and deregistrations for the
/// source table, straight through that service's own API.
async fn given_live_settings(case: &Case, emulator: &Emulator) {
    for (capacity, minimum, maximum, target_value) in &case.registered_directly {
        let identity = json!({
            "ServiceNamespace": keyspaces::AUTO_SCALING_NAMESPACE,
            "ResourceId": resource_id(case.keyspace, case.source),
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
            "case {:?}: expected RegisterScalableTarget to answer 200, got {status} {body}",
            case.name
        );
        let mut put = identity.clone();
        put["PolicyName"] = json!(case
            .direct_policy_name
            .map(str::to_string)
            .unwrap_or_else(|| expected_policy_name(case.keyspace, case.source)));
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
            "case {:?}: expected PutScalingPolicy to answer 200, got {status} {body}",
            case.name
        );
    }
    for capacity in &case.deregistered {
        let (status, body) = emulator
            .auto_scaling(
                "DeregisterScalableTarget",
                json!({
                    "ServiceNamespace": keyspaces::AUTO_SCALING_NAMESPACE,
                    "ResourceId": resource_id(case.keyspace, case.source),
                    "ScalableDimension": capacity.scalable_dimension(),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DeregisterScalableTarget to answer 200, got {status} {body}",
            case.name
        );
    }
}

/// RULE: the auto scaling settings `RestoreTable` reproduces for the restored table are
/// the ones Application Auto Scaling holds for the source table's capacities as of the
/// call — the same settings `GetTableAutoScalingSettings` reports for that source table,
/// whichever service wrote them there — so a capacity re-tuned through Application Auto
/// Scaling carries over its CURRENT window and target value, and a capacity with no
/// scalable target carries nothing over.
#[tokio::test]
async fn restore_reproduces_the_auto_scaling_settings_the_source_has_now() {
    let emulator = Emulator::start();
    for case in cases() {
        given_source_table(&case, &emulator).await;
        given_live_settings(&case, &emulator).await;

        // The reference point: what the SOURCE table's settings are at the instant the
        // restore is called.
        assert_reported(
            case.name,
            "source before the restore",
            &emulator,
            case.keyspace,
            case.source,
            &case.expect,
        )
        .await;

        let (status, body) = emulator
            .keyspaces(
                "RestoreTable",
                json!({
                    "sourceKeyspaceName": case.keyspace,
                    "sourceTableName": case.source,
                    "targetKeyspaceName": case.keyspace,
                    "targetTableName": case.restored,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected RestoreTable to answer 200, got {status} {body}",
            case.name
        );
        let suffix = format!("/table/{}", case.restored);
        assert!(
            body["restoredTableARN"]
                .as_str()
                .is_some_and(|arn| arn.ends_with(&suffix)),
            "case {:?}: expected restoredTableARN to name the restored table, got {body}",
            case.name
        );
        tokio::time::sleep(settle_wait()).await;

        // The restored table, read back through `GetTable` before anything is believed
        // about it.
        let (status, body) = emulator
            .keyspaces(
                "GetTable",
                json!({"keyspaceName": case.keyspace, "tableName": case.restored}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetTable on the restored table to answer 200, \
             got {status} {body}",
            case.name
        );
        assert_eq!(
            body["status"],
            json!(keyspaces::TABLE_STATUS_ACTIVE),
            "case {:?}: expected the restored table to have settled, got {body}",
            case.name
        );

        // What the restore reproduced, from both sides: the Amazon Keyspaces report of the
        // restored table, and the Application Auto Scaling resources that report stands on.
        assert_reported(
            case.name,
            "restored table",
            &emulator,
            case.keyspace,
            case.restored,
            &case.expect,
        )
        .await;
        assert_held_by_auto_scaling(
            case.name,
            "restored table",
            &emulator,
            case.keyspace,
            case.restored,
            &case.expect,
        )
        .await;

        // And the source table is left exactly as it was: a restore reads it.
        assert_reported(
            case.name,
            "source after the restore",
            &emulator,
            case.keyspace,
            case.source,
            &case.expect,
        )
        .await;
    }
}
