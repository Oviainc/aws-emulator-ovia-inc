// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateMonitor`, driven over the service's own `restJson1`
//! wire protocol through the shared harness (`tests/all/harness.rs`).
//!
//! The RULE under test: an update edits the addressed monitor's `localResources`
//! and `remoteResources` as SETS keyed by an entry's `type` + `identifier` — the
//! request's removals land first and then its additions — leaves every other
//! member of the record alone, moves `modifiedAt` exactly when the record changed,
//! and answers with the stored record, so the next `GetMonitor` reports the same
//! thing. The issue's recorded call (adding one remote resource) is ONE row of the
//! table below.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_networkflowmonitor_networkflowmonitor::state::MAX_MONITOR_RESOURCES;

use crate::harness::{client_token, Emulator, PAST_PROVISIONING};

/// A monitor resource list as the wire carries it: one `{type, identifier}` entry
/// per pair. The types are the ones the model declares for
/// `MonitorLocalResourceType` / `MonitorRemoteResourceType`.
fn resources(entries: &[(&str, &str)]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|(resource_type, identifier)| {
                json!({ "type": resource_type, "identifier": identifier })
            })
            .collect(),
    )
}

/// The `(type, identifier)` identities a response's resource list reports, as a
/// set — the identity the lists are edited by. Also answers how many entries the
/// list carried, so a test can tell a set from a list holding duplicates.
fn identities(list: &Value) -> (BTreeSet<(String, String)>, usize) {
    let entries = list.as_array().cloned().unwrap_or_default();
    let count = entries.len();
    let set = entries
        .iter()
        .map(|entry| {
            (
                entry
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                entry
                    .get("identifier")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    (set, count)
}

/// A set of `(type, identifier)` pairs to expect.
fn expected(entries: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    entries
        .iter()
        .map(|(resource_type, identifier)| (resource_type.to_string(), identifier.to_string()))
        .collect()
}

/// `count` distinct resource entries of one type, identified by `prefix` plus an
/// index — the caller-chosen free text an entry's `identifier` is.
fn distinct_resources(resource_type: &str, prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "type": resource_type, "identifier": format!("{prefix}-{index}") }))
            .collect(),
    )
}

/// How many entries a response's resource list carries.
fn count_of(list: &Value) -> usize {
    list.as_array().map_or(0, Vec::len)
}

/// One row of the update table: the monitor to create, the update to apply, and
/// the resource sets the record must hold afterwards.
struct Row {
    case: &'static str,
    name: &'static str,
    local: Vec<(&'static str, &'static str)>,
    remote: Vec<(&'static str, &'static str)>,
    update: Value,
    expect_local: Vec<(&'static str, &'static str)>,
    expect_remote: Vec<(&'static str, &'static str)>,
    /// Whether the update changes the stored record, which is what decides
    /// whether `modifiedAt` moves.
    changes_record: bool,
}

/// The table: the rows differ along WHICH list is edited, whether the request
/// adds, removes or does both, how many entries it names, the resource type, and
/// whether the named entries are already present or absent.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded shape: one remote resource added to a monitor with none",
            name: "add-one-remote",
            local: vec![("AWS::EC2::VPC", "vpc-0a1b2c3d")],
            remote: vec![],
            update: json!({ "remoteResourcesToAdd": resources(&[("AWS::AWSService", "DynamoDB")]) }),
            expect_local: vec![("AWS::EC2::VPC", "vpc-0a1b2c3d")],
            expect_remote: vec![("AWS::AWSService", "DynamoDB")],
            changes_record: true,
        },
        Row {
            case: "three local resources of three types added at once",
            name: "add-three-local",
            local: vec![("AWS::EC2::VPC", "vpc-0a1b2c3d")],
            remote: vec![],
            update: json!({
                "localResourcesToAdd": resources(&[
                    ("AWS::EC2::Subnet", "subnet-1111"),
                    ("AWS::AvailabilityZone", "us-west-2b"),
                    ("AWS::EKS::Cluster", "cluster-a"),
                ])
            }),
            expect_local: vec![
                ("AWS::EC2::VPC", "vpc-0a1b2c3d"),
                ("AWS::EC2::Subnet", "subnet-1111"),
                ("AWS::AvailabilityZone", "us-west-2b"),
                ("AWS::EKS::Cluster", "cluster-a"),
            ],
            expect_remote: vec![],
            changes_record: true,
        },
        Row {
            case: "one of three local resources removed",
            name: "remove-one-local",
            local: vec![
                ("AWS::EC2::VPC", "vpc-1"),
                ("AWS::EC2::Subnet", "subnet-2"),
                ("AWS::Region", "eu-west-1"),
            ],
            remote: vec![],
            update: json!({ "localResourcesToRemove": resources(&[("AWS::EC2::Subnet", "subnet-2")]) }),
            expect_local: vec![("AWS::EC2::VPC", "vpc-1"), ("AWS::Region", "eu-west-1")],
            expect_remote: vec![],
            changes_record: true,
        },
        Row {
            case: "every remote resource removed: the list empties",
            name: "remove-all-remote",
            local: vec![("AWS::EC2::VPC", "vpc-1")],
            remote: vec![("AWS::AWSService", "S3"), ("AWS::Region", "us-east-2")],
            update: json!({
                "remoteResourcesToRemove": resources(&[
                    ("AWS::AWSService", "S3"),
                    ("AWS::Region", "us-east-2"),
                ])
            }),
            expect_local: vec![("AWS::EC2::VPC", "vpc-1")],
            expect_remote: vec![],
            changes_record: true,
        },
        Row {
            case: "both lists edited in one request, adding and removing at once",
            name: "edit-both-lists",
            local: vec![("AWS::EC2::VPC", "vpc-1"), ("AWS::EC2::Subnet", "subnet-2")],
            remote: vec![("AWS::AWSService", "S3")],
            update: json!({
                "localResourcesToAdd": resources(&[("AWS::AvailabilityZone", "us-west-2a")]),
                "localResourcesToRemove": resources(&[("AWS::EC2::Subnet", "subnet-2")]),
                "remoteResourcesToAdd": resources(&[("AWS::EC2::VPC", "vpc-remote")]),
                "remoteResourcesToRemove": resources(&[("AWS::AWSService", "S3")]),
            }),
            expect_local: vec![
                ("AWS::EC2::VPC", "vpc-1"),
                ("AWS::AvailabilityZone", "us-west-2a"),
            ],
            expect_remote: vec![("AWS::EC2::VPC", "vpc-remote")],
            changes_record: true,
        },
        Row {
            case: "adding a resource the monitor already holds is a no-op, not a duplicate",
            name: "add-already-present",
            local: vec![("AWS::EC2::VPC", "vpc-1")],
            remote: vec![("AWS::AWSService", "S3")],
            update: json!({
                "localResourcesToAdd": resources(&[("AWS::EC2::VPC", "vpc-1")]),
                "remoteResourcesToAdd": resources(&[("AWS::AWSService", "S3")]),
            }),
            expect_local: vec![("AWS::EC2::VPC", "vpc-1")],
            expect_remote: vec![("AWS::AWSService", "S3")],
            changes_record: false,
        },
        Row {
            case: "removing a resource the monitor does not hold changes nothing",
            name: "remove-absent",
            local: vec![("AWS::EC2::VPC", "vpc-1")],
            remote: vec![],
            update: json!({
                "localResourcesToRemove": resources(&[("AWS::EC2::Subnet", "subnet-absent")]),
                "remoteResourcesToRemove": resources(&[("AWS::AWSService", "S3")]),
            }),
            expect_local: vec![("AWS::EC2::VPC", "vpc-1")],
            expect_remote: vec![],
            changes_record: false,
        },
        Row {
            case: "one resource named by both the add and the remove list ends present",
            name: "add-and-remove-one",
            local: vec![("AWS::EC2::VPC", "vpc-1")],
            remote: vec![("AWS::AWSService", "S3")],
            update: json!({
                "remoteResourcesToAdd": resources(&[("AWS::AWSService", "S3")]),
                "remoteResourcesToRemove": resources(&[("AWS::AWSService", "S3")]),
            }),
            expect_local: vec![("AWS::EC2::VPC", "vpc-1")],
            expect_remote: vec![("AWS::AWSService", "S3")],
            changes_record: false,
        },
        Row {
            case: "an update naming no resource at all leaves the monitor alone",
            name: "empty-update",
            local: vec![("AWS::EC2::VPC", "vpc-1")],
            remote: vec![("AWS::Region", "us-east-1")],
            update: json!({}),
            expect_local: vec![("AWS::EC2::VPC", "vpc-1")],
            expect_remote: vec![("AWS::Region", "us-east-1")],
            changes_record: false,
        },
        Row {
            case: "a monitor carrying 12 local resources: the count dimension",
            name: "many-local",
            local: vec![
                ("AWS::EC2::Subnet", "subnet-00"),
                ("AWS::EC2::Subnet", "subnet-01"),
                ("AWS::EC2::Subnet", "subnet-02"),
                ("AWS::EC2::Subnet", "subnet-03"),
                ("AWS::EC2::Subnet", "subnet-04"),
                ("AWS::EC2::Subnet", "subnet-05"),
                ("AWS::EC2::Subnet", "subnet-06"),
                ("AWS::EC2::Subnet", "subnet-07"),
                ("AWS::EC2::Subnet", "subnet-08"),
                ("AWS::EC2::Subnet", "subnet-09"),
                ("AWS::EC2::Subnet", "subnet-10"),
                ("AWS::EC2::Subnet", "subnet-11"),
            ],
            remote: vec![],
            update: json!({
                "localResourcesToRemove": resources(&[
                    ("AWS::EC2::Subnet", "subnet-00"),
                    ("AWS::EC2::Subnet", "subnet-11"),
                ]),
                "localResourcesToAdd": resources(&[("AWS::Region", "ap-south-1")]),
            }),
            expect_local: vec![
                ("AWS::EC2::Subnet", "subnet-01"),
                ("AWS::EC2::Subnet", "subnet-02"),
                ("AWS::EC2::Subnet", "subnet-03"),
                ("AWS::EC2::Subnet", "subnet-04"),
                ("AWS::EC2::Subnet", "subnet-05"),
                ("AWS::EC2::Subnet", "subnet-06"),
                ("AWS::EC2::Subnet", "subnet-07"),
                ("AWS::EC2::Subnet", "subnet-08"),
                ("AWS::EC2::Subnet", "subnet-09"),
                ("AWS::EC2::Subnet", "subnet-10"),
                ("AWS::Region", "ap-south-1"),
            ],
            expect_remote: vec![],
            changes_record: true,
        },
    ]
}

#[tokio::test]
async fn an_update_edits_the_stored_resource_lists_as_sets() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    for row in rows() {
        let created = emulator
            .create_monitor(json!({
                "monitorName": row.name,
                "scopeArn": scope_arn,
                "localResources": resources(&row.local),
                "remoteResources": resources(&row.remote),
                "tags": { "suite": "monitor-update" }
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the monitor under test must be created, got {} with body {}",
            row.case, created.status, created.body
        );

        let updated = emulator.update_monitor(row.name, row.update.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 from UpdateMonitor, got {} with body {}",
            row.case, updated.status, updated.body
        );

        for (member, want) in [
            ("localResources", expected(&row.expect_local)),
            ("remoteResources", expected(&row.expect_remote)),
        ] {
            let (got, count) = identities(updated.member(member));
            assert_eq!(
                got, want,
                "case {:?}: expected {member} {want:?}, got {:?}",
                row.case,
                updated.member(member)
            );
            assert_eq!(
                count,
                want.len(),
                "case {:?}: {member} is a set, so it holds no duplicate entry: got {:?}",
                row.case,
                updated.member(member)
            );
        }

        // Every other member of the record is the create's, and the update's answer
        // is the state: the following read reports exactly what the update answered.
        for member in ["monitorArn", "monitorName", "createdAt", "tags"] {
            assert_eq!(
                updated.member(member),
                created.member(member),
                "case {:?}: an update leaves {member} alone, got {:?} after {:?}",
                row.case,
                updated.member(member),
                created.member(member)
            );
        }

        let read = emulator.get_monitor(row.name).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the updated monitor must still be readable, got {} with body {}",
            row.case, read.status, read.body
        );
        for member in ["localResources", "remoteResources", "monitorArn", "tags"] {
            assert_eq!(
                read.member(member),
                updated.member(member),
                "case {:?}: GetMonitor reports what the update stored for {member}: got {:?} after {:?}",
                row.case,
                read.member(member),
                updated.member(member)
            );
        }
    }
}

#[tokio::test]
async fn modified_at_moves_exactly_when_the_record_changed() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    for row in rows() {
        let name = format!("{}-timestamps", row.name);
        let created = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": resources(&row.local),
                "remoteResources": resources(&row.remote)
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the monitor under test must be created, got {} with body {}",
            row.case, created.status, created.body
        );
        // Past the provisioning transition, so the only thing that can move
        // `modifiedAt` from here on is the update itself.
        tokio::time::sleep(PAST_PROVISIONING).await;
        let settled = emulator.get_monitor(&name).await;
        let before = settled.number("modifiedAt");

        let updated = emulator.update_monitor(&name, row.update.clone()).await;
        assert_eq!(
            updated.status, 200,
            "case {:?}: expected 200 from UpdateMonitor, got {} with body {}",
            row.case, updated.status, updated.body
        );
        let after = updated.number("modifiedAt");
        if row.changes_record {
            assert!(
                after > before,
                "case {:?}: an update that changed the record moves modifiedAt past {before}, got {after}"
                , row.case
            );
        } else {
            assert_eq!(
                after, before,
                "case {:?}: an update that changed nothing leaves modifiedAt at {before}, got {after}",
                row.case
            );
        }
        assert_eq!(
            updated.number("createdAt"),
            created.number("createdAt"),
            "case {:?}: createdAt never moves, got {} after {}",
            row.case,
            updated.number("createdAt"),
            created.number("createdAt")
        );
        // The lifecycle status is the clock's, not the update's: a monitor the clock
        // already owes ACTIVE stays ACTIVE across an update.
        assert_eq!(
            updated.text("monitorStatus"),
            settled.text("monitorStatus"),
            "case {:?}: an update does not restart the lifecycle, got {:?} after {:?}",
            row.case,
            updated.text("monitorStatus"),
            settled.text("monitorStatus")
        );
    }
}

#[tokio::test]
async fn one_client_token_applies_one_update() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let created = emulator
        .create_monitor(json!({
            "monitorName": "idempotent-updates",
            "scopeArn": scope_arn,
            "localResources": resources(&[("AWS::EC2::VPC", "vpc-1")])
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the monitor under test must be created, got {} with body {}",
        created.status, created.body
    );
    tokio::time::sleep(PAST_PROVISIONING).await;

    // Three distinct tokens, each adding one remote resource, then each REPLAYED:
    // the replay must neither re-apply the addition nor move `modifiedAt`.
    let additions = [
        ("AWS::AWSService", "S3"),
        ("AWS::Region", "us-east-2"),
        ("AWS::EC2::Subnet", "subnet-remote"),
    ];
    for (index, addition) in additions.iter().enumerate() {
        let token = client_token(100 + index as u32);
        let body = json!({
            "clientToken": token,
            "remoteResourcesToAdd": resources(&[*addition])
        });

        let first = emulator.update_monitor("idempotent-updates", body.clone()).await;
        assert_eq!(
            first.status, 200,
            "case {addition:?}: expected 200 from the first update, got {} with body {}",
            first.status, first.body
        );
        let (after_first, _) = identities(first.member("remoteResources"));
        assert!(
            after_first.contains(&(addition.0.to_string(), addition.1.to_string())),
            "case {addition:?}: the addition must be stored, got {:?}",
            first.member("remoteResources")
        );

        // Something else happens to the monitor between the call and its replay, so
        // the replay cannot be answered by "nothing changed anyway".
        let removal = emulator
            .update_monitor(
                "idempotent-updates",
                json!({ "remoteResourcesToRemove": resources(&[*addition]) }),
            )
            .await;
        assert_eq!(
            removal.status, 200,
            "case {addition:?}: expected 200 from the removal, got {} with body {}",
            removal.status, removal.body
        );
        let expected_body = emulator.get_monitor("idempotent-updates").await.body;

        let replay = emulator.update_monitor("idempotent-updates", body).await;
        assert_eq!(
            replay.status, 200,
            "case {addition:?}: expected 200 from the replay, got {} with body {}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.body, expected_body,
            "case {addition:?}: replaying one clientToken applies the update once, got {} instead of {}",
            replay.body, expected_body
        );

        // A DIFFERENT token carrying the same change is a new request, and applies.
        let reapplied = emulator
            .update_monitor(
                "idempotent-updates",
                json!({
                    "clientToken": client_token(200 + index as u32),
                    "remoteResourcesToAdd": resources(&[*addition])
                }),
            )
            .await;
        let (after_reapply, _) = identities(reapplied.member("remoteResources"));
        assert!(
            after_reapply.contains(&(addition.0.to_string(), addition.1.to_string())),
            "case {addition:?}: a new clientToken applies the addition again, got {:?}",
            reapplied.member("remoteResources")
        );

        // And the SAME token carrying a DIFFERENT change is refused, rather than
        // answered with a success for a write nothing applied. Three shapes of
        // "different": another member, another resource, and one extra entry.
        let before_mismatch = emulator.get_monitor("idempotent-updates").await.body;
        let mismatches = [
            (
                "the same resource moved to the other member",
                json!({
                    "clientToken": token,
                    "remoteResourcesToRemove": resources(&[*addition])
                }),
            ),
            (
                "another resource under the same token",
                json!({
                    "clientToken": token,
                    "remoteResourcesToAdd": resources(&[("AWS::Region", "eu-central-1")])
                }),
            ),
            (
                "the same resource plus one more",
                json!({
                    "clientToken": token,
                    "remoteResourcesToAdd":
                        resources(&[*addition, ("AWS::Region", "eu-central-1")])
                }),
            ),
        ];
        for (case, body) in mismatches {
            let refused = emulator.update_monitor("idempotent-updates", body).await;
            assert_eq!(
                refused.status, 400,
                "case {addition:?}/{case:?}: a token reused for a different change is refused, got {} with body {}",
                refused.status, refused.body
            );
            assert!(
                refused.error_type.contains("ValidationException"),
                "case {addition:?}/{case:?}: expected ValidationException, got {:?} with body {}",
                refused.error_type,
                refused.body
            );
            let after_refusal = emulator.get_monitor("idempotent-updates").await.body;
            assert_eq!(
                after_refusal, before_mismatch,
                "case {addition:?}/{case:?}: a refused update changes nothing, got {after_refusal} instead of {before_mismatch}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_of_a_monitor_the_store_does_not_hold_is_404() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    // One real monitor exists, so a 404 is about the name and not an empty store.
    let created = emulator
        .create_monitor(json!({
            "monitorName": "a-real-monitor",
            "scopeArn": scope_arn,
            "localResources": resources(&[("AWS::EC2::VPC", "vpc-1")])
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "the monitor under test must be created, got {} with body {}",
        created.status, created.body
    );

    // Names inside `ResourceName`'s pattern `^[a-zA-Z0-9_.-]+$` that the store does
    // not hold: an unknown name, a near-miss of the real one, a case variant (names
    // are case-sensitive) and the maximum length the model allows (255).
    let long_name = "n".repeat(255);
    let cases = [
        "no-such-monitor",
        "a-real-monitor.2",
        "A-REAL-MONITOR",
        "a_real_monitor",
        long_name.as_str(),
    ];
    for case in cases {
        let answer = emulator
            .update_monitor(
                case,
                json!({ "remoteResourcesToAdd": resources(&[("AWS::AWSService", "S3")]) }),
            )
            .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for a monitor the store does not hold, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} with body {}",
            answer.error_type,
            answer.body
        );
    }

    // And the monitor that does exist was not touched by any of those refusals.
    let read = emulator.get_monitor("a-real-monitor").await;
    let (local, _) = identities(read.member("localResources"));
    assert_eq!(
        local,
        expected(&[("AWS::EC2::VPC", "vpc-1")]),
        "a refused update changes no other monitor, got {:?}",
        read.member("localResources")
    );
    let (remote, count) = identities(read.member("remoteResources"));
    assert!(
        remote.is_empty() && count == 0,
        "a refused update adds nothing to another monitor, got {:?}",
        read.member("remoteResources")
    );
}

#[tokio::test]
async fn a_create_stores_its_resource_lists_as_sets() {
    // The set rule holds for a record's whole life, not only once an update has
    // touched it: a create carrying the same resource twice stores it once, so the
    // record a create mints is the record an update would have produced.
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;
    let rows = [
        (
            "one resource repeated twice",
            resources(&[("AWS::EC2::VPC", "vpc-1"), ("AWS::EC2::VPC", "vpc-1")]),
            vec![("AWS::EC2::VPC", "vpc-1")],
        ),
        (
            "two resources, the first repeated at the end",
            resources(&[
                ("AWS::EC2::VPC", "vpc-1"),
                ("AWS::EC2::Subnet", "subnet-2"),
                ("AWS::EC2::VPC", "vpc-1"),
            ]),
            vec![("AWS::EC2::VPC", "vpc-1"), ("AWS::EC2::Subnet", "subnet-2")],
        ),
        (
            "one identifier under two types: two different resources",
            resources(&[("AWS::EC2::VPC", "shared-id"), ("AWS::Region", "shared-id")]),
            vec![("AWS::EC2::VPC", "shared-id"), ("AWS::Region", "shared-id")],
        ),
    ];
    for (index, (case, list, want)) in rows.into_iter().enumerate() {
        let name = format!("deduped-{index}");
        let created = emulator
            .create_monitor(json!({
                "monitorName": name,
                "scopeArn": scope_arn,
                "localResources": list
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateMonitor, got {} with body {}",
            created.status, created.body
        );
        let (got, count) = identities(created.member("localResources"));
        assert_eq!(
            got,
            expected(&want),
            "case {case:?}: expected localResources {:?}, got {:?}",
            expected(&want),
            created.member("localResources")
        );
        assert_eq!(
            count,
            want.len(),
            "case {case:?}: the stored list holds each resource once, got {:?}",
            created.member("localResources")
        );
        let read = emulator.get_monitor(&name).await;
        assert_eq!(
            read.member("localResources"),
            created.member("localResources"),
            "case {case:?}: GetMonitor reports what the create stored, got {:?} after {:?}",
            read.member("localResources"),
            created.member("localResources")
        );
    }
}

#[tokio::test]
async fn a_resource_list_cannot_grow_past_the_store_s_ceiling() {
    let emulator = Emulator::new();
    let scope_arn = emulator.scope_for_monitors().await;

    // A create AT the ceiling is accepted, and one past it is refused, so the
    // ceiling cannot be sidestepped by creating the oversized monitor instead of
    // growing one.
    let at_ceiling = emulator
        .create_monitor(json!({
            "monitorName": "at-the-ceiling",
            "scopeArn": scope_arn,
            "localResources": distinct_resources("AWS::EC2::Subnet", "subnet", MAX_MONITOR_RESOURCES)
        }))
        .await;
    assert_eq!(
        at_ceiling.status, 200,
        "case {MAX_MONITOR_RESOURCES} local resources: a create at the ceiling is accepted, got {} with body {}",
        at_ceiling.status, at_ceiling.body
    );
    assert_eq!(
        count_of(at_ceiling.member("localResources")),
        MAX_MONITOR_RESOURCES,
        "case {MAX_MONITOR_RESOURCES} local resources: the whole list is stored, got {}",
        count_of(at_ceiling.member("localResources"))
    );

    let past_ceiling = emulator
        .create_monitor(json!({
            "monitorName": "past-the-ceiling",
            "scopeArn": scope_arn,
            "localResources": distinct_resources("AWS::EC2::Subnet", "subnet", MAX_MONITOR_RESOURCES + 1)
        }))
        .await;
    assert_eq!(
        past_ceiling.status, 400,
        "case {} local resources: a create past the ceiling is refused, got {} with body {}",
        MAX_MONITOR_RESOURCES + 1,
        past_ceiling.status,
        past_ceiling.body
    );
    assert!(
        past_ceiling.error_type.contains("ValidationException"),
        "case {} local resources: expected ValidationException, got {:?} with body {}",
        MAX_MONITOR_RESOURCES + 1,
        past_ceiling.error_type,
        past_ceiling.body
    );
    let absent = emulator.get_monitor("past-the-ceiling").await;
    assert_eq!(
        absent.status, 404,
        "a refused create stores no monitor, got {} with body {}",
        absent.status, absent.body
    );

    // Every update that would leave a list past the ceiling is refused, with the
    // record left exactly as it was; one that keeps the list at the ceiling is not.
    let before = emulator.get_monitor("at-the-ceiling").await;
    let refusals = [
        (
            "one addition onto a full list",
            json!({ "localResourcesToAdd": distinct_resources("AWS::Region", "us-west", 1) }),
        ),
        (
            "two additions against one removal: a net overflow of one",
            json!({
                "localResourcesToRemove": distinct_resources("AWS::EC2::Subnet", "subnet", 1),
                "localResourcesToAdd": distinct_resources("AWS::Region", "region", 2),
            }),
        ),
        (
            "an addition list longer than the ceiling, onto an empty list",
            json!({
                "remoteResourcesToAdd":
                    distinct_resources("AWS::Region", "region", MAX_MONITOR_RESOURCES + 1)
            }),
        ),
    ];
    for (case, body) in refusals {
        let answer = emulator.update_monitor("at-the-ceiling", body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a list past the ceiling, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} with body {}",
            answer.error_type,
            answer.body
        );
        let after = emulator.get_monitor("at-the-ceiling").await;
        assert_eq!(
            after.body, before.body,
            "case {case:?}: a refused update changes nothing, got {} instead of {}",
            after.body, before.body
        );
    }

    // A removal list longer than the ceiling is refused too: no list a monitor may
    // hold can name more entries than a monitor holds, and bounding it keeps what
    // the store remembers for a `clientToken` derived from a bounded request.
    let long_removal = emulator
        .update_monitor(
            "at-the-ceiling",
            json!({
                "remoteResourcesToRemove":
                    distinct_resources("AWS::AWSService", "svc", MAX_MONITOR_RESOURCES + 1)
            }),
        )
        .await;
    assert_eq!(
        long_removal.status, 400,
        "a removal list longer than the ceiling is refused, got {} with body {}",
        long_removal.status, long_removal.body
    );
    assert!(
        long_removal.error_type.contains("ValidationException"),
        "expected ValidationException for an oversized removal list, got {:?} with body {}",
        long_removal.error_type,
        long_removal.body
    );
    let unchanged = emulator.get_monitor("at-the-ceiling").await;
    assert_eq!(
        unchanged.body, before.body,
        "a refused update changes nothing, got {} instead of {}",
        unchanged.body, before.body
    );

    // A removal list AT the ceiling is accepted, and removing resources the monitor
    // does not hold changes nothing.
    let removal_at_ceiling = emulator
        .update_monitor(
            "at-the-ceiling",
            json!({
                "remoteResourcesToRemove":
                    distinct_resources("AWS::AWSService", "svc", MAX_MONITOR_RESOURCES)
            }),
        )
        .await;
    assert_eq!(
        removal_at_ceiling.status, 200,
        "a removal list at the ceiling is accepted, got {} with body {}",
        removal_at_ceiling.status, removal_at_ceiling.body
    );
    assert_eq!(
        removal_at_ceiling.member("localResources"),
        before.member("localResources"),
        "removing resources the monitor does not hold changes nothing, got {:?}",
        removal_at_ceiling.member("localResources")
    );

    // Swapping one entry for another keeps the list at the ceiling and is accepted.
    let swapped = emulator
        .update_monitor(
            "at-the-ceiling",
            json!({
                "localResourcesToRemove": distinct_resources("AWS::EC2::Subnet", "subnet", 1),
                "localResourcesToAdd": distinct_resources("AWS::Region", "region", 1),
            }),
        )
        .await;
    assert_eq!(
        swapped.status, 200,
        "an update that keeps the list at the ceiling is accepted, got {} with body {}",
        swapped.status, swapped.body
    );
    assert_eq!(
        count_of(swapped.member("localResources")),
        MAX_MONITOR_RESOURCES,
        "the list stays at the ceiling, got {}",
        count_of(swapped.member("localResources"))
    );

    // And growing a list in batches stops at the ceiling rather than past it: the
    // bound holds across calls, not just within one.
    let grown = emulator
        .create_monitor(json!({
            "monitorName": "grown-in-batches",
            "scopeArn": scope_arn,
            "localResources": distinct_resources("AWS::EC2::VPC", "vpc", 1)
        }))
        .await;
    assert_eq!(
        grown.status, 200,
        "the monitor under test must be created, got {} with body {}",
        grown.status, grown.body
    );
    let batch = MAX_MONITOR_RESOURCES / 4;
    let mut accepted = 0usize;
    for round in 0..6 {
        let answer = emulator
            .update_monitor(
                "grown-in-batches",
                json!({
                    "remoteResourcesToAdd":
                        distinct_resources("AWS::AWSService", &format!("svc{round}"), batch)
                }),
            )
            .await;
        if answer.status == 200 {
            accepted += batch;
            assert_eq!(
                count_of(answer.member("remoteResources")),
                accepted,
                "round {round}: every accepted addition is stored once, got {}",
                count_of(answer.member("remoteResources"))
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "round {round}: a batch past the ceiling is refused with 400, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
    let final_read = emulator.get_monitor("grown-in-batches").await;
    assert!(
        count_of(final_read.member("remoteResources")) <= MAX_MONITOR_RESOURCES,
        "a list grown in batches stops at the ceiling {MAX_MONITOR_RESOURCES}, got {}",
        count_of(final_read.member("remoteResources"))
    );
    assert_eq!(
        count_of(final_read.member("remoteResources")),
        accepted,
        "the store holds exactly the additions it accepted, got {}",
        count_of(final_read.member("remoteResources"))
    );
}
