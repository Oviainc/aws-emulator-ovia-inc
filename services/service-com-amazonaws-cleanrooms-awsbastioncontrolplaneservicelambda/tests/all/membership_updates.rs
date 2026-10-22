// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateMembership`.
//!
//! The rule under test: `UpdateMembership` is a PATCH over the stored membership —
//! every member the request carries replaces the stored one, every member it omits is
//! left exactly as stored, `updateTime` is restamped while `createTime` and the
//! collaboration-derived members never move, and the response is the record a later
//! read returns. A refused update persists nothing.
//!
//! The tables vary what the rule generalises over: which member the request carries,
//! how many it carries at once, the value it carries (every enum value of each log
//! status, both result configurations, nested payment sub-structures), and the
//! member-ability set that decides the payment responsibility a member may accept.
//! Requests go over the wire through the same `MultiServiceRouter` the production
//! router uses.

use serde_json::{json, Value};

use super::cleanrooms_collaboration_lifecycle::{
    error_code, minimal_collaboration, Emulator, ALL_LOG_STATUSES,
};

/// Every member the modelled `Membership` shape declares except `updateTime` (restamped
/// by every successful update), from `smithy_query(query="shape", shape="Membership")`.
/// A row asserts that each of these either carries the value the request sent for it or
/// is untouched.
const MEMBERSHIP_MEMBERS: [&str; 17] = [
    "id",
    "arn",
    "collaborationArn",
    "collaborationId",
    "collaborationCreatorAccountId",
    "collaborationCreatorDisplayName",
    "collaborationName",
    "createTime",
    "status",
    "memberAbilities",
    "mlMemberAbilities",
    "queryLogStatus",
    "jobLogStatus",
    "defaultResultConfiguration",
    "defaultJobResultConfiguration",
    "paymentConfiguration",
    "isMetricsEnabled",
];

/// The baseline membership every row of the PATCH table starts from: query logging on,
/// job logging off, neither result configuration set, and the payment responsibilities
/// the collaboration assigns a creator whose only ability is `CAN_QUERY`.
fn baseline_membership_body(collaboration_id: &str) -> Value {
    json!({
        "collaborationIdentifier": collaboration_id,
        "queryLogStatus": "ENABLED",
        "jobLogStatus": "DISABLED",
    })
}

/// The payment configuration that baseline membership reports: responsible for query
/// compute (it holds `CAN_QUERY`, the default payer per
/// `QueryComputePaymentConfig$isResponsible`), not for job compute (no `CAN_RUN_JOB`),
/// and not for any ML cost.
fn baseline_payment_configuration() -> Value {
    json!({
        "queryCompute": { "isResponsible": true },
        "machineLearning": {
            "modelTraining": { "isResponsible": false },
            "modelInference": { "isResponsible": false },
            "syntheticDataGeneration": { "isResponsible": false },
        },
        "jobCompute": { "isResponsible": false },
    })
}

/// A membership-side protected QUERY result configuration
/// (`MembershipProtectedQueryResultConfiguration`), with `ResultFormat` `format`.
fn query_result_configuration(format: &str, key_prefix: Option<&str>) -> Value {
    let mut s3 = json!({ "resultFormat": format, "bucket": "cleanrooms-results" });
    if let Some(prefix) = key_prefix {
        s3["keyPrefix"] = json!(prefix);
    }
    json!({
        "outputConfiguration": { "s3": s3 },
        "roleArn": "arn:aws:iam::000000000000:role/clean-rooms-results",
    })
}

/// A membership-side protected JOB result configuration
/// (`MembershipProtectedJobResultConfiguration`, whose `roleArn` is required).
fn job_result_configuration(bucket: &str) -> Value {
    json!({
        "outputConfiguration": { "s3": { "bucket": bucket, "keyPrefix": "jobs" } },
        "roleArn": "arn:aws:iam::000000000000:role/clean-rooms-jobs",
    })
}

/// Create a collaboration whose creator holds `abilities`, join it, and return
/// `(membership id, the created membership)`.
async fn seeded_membership(
    emulator: &Emulator,
    label: &str,
    abilities: &[&str],
) -> (String, Value) {
    let mut collaboration = minimal_collaboration(label);
    collaboration["creatorMemberAbilities"] = json!(abilities);
    let (status, created) = emulator.create_collaboration(collaboration).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateCollaboration 200, got {status} with {created}"
    );
    let collaboration_id = created["collaboration"]["id"]
        .as_str()
        .expect("the created collaboration carries an id")
        .to_string();

    let (status, created) = emulator
        .create_membership(baseline_membership_body(&collaboration_id))
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected CreateMembership 200, got {status} with {created}"
    );
    let membership = created["membership"].clone();
    let membership_id = membership["id"]
        .as_str()
        .expect("the created membership carries an id")
        .to_string();
    (membership_id, membership)
}

/// `PATCH /memberships/{id}` — the `UpdateMembership` binding
/// (`smithy_query(query="operation", operation="UpdateMembership")`).
async fn update_membership(emulator: &Emulator, id: &str, body: Value) -> (u16, Value) {
    emulator
        .call("PATCH", &format!("/memberships/{id}"), Some(body))
        .await
}

/// The epoch-seconds value of a membership timestamp member.
fn timestamp(membership: &Value, member: &str, case: &str) -> f64 {
    membership[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: {member} must be a timestamp, got {membership}"))
}

/// One row of the PATCH table: the update body and the membership members it replaces.
struct UpdateCase {
    label: &'static str,
    update: Value,
    /// `Membership` member -> the value it must carry after the update. Every other
    /// member must equal the baseline.
    expected: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn update_membership_replaces_only_the_members_the_request_carries() {
    let emulator = Emulator::new();

    let cases = vec![
        UpdateCase {
            label: "empty update",
            update: json!({}),
            expected: vec![],
        },
        UpdateCase {
            // `MembershipQueryLogStatus`: ENABLED | DISABLED — both values, so the row
            // set covers the whole enum rather than the one value the recording used.
            label: "queryLogStatus flipped to DISABLED",
            update: json!({ "queryLogStatus": ALL_LOG_STATUSES[1] }),
            expected: vec![("queryLogStatus", json!(ALL_LOG_STATUSES[1]))],
        },
        UpdateCase {
            label: "queryLogStatus restated as ENABLED",
            update: json!({ "queryLogStatus": ALL_LOG_STATUSES[0] }),
            expected: vec![("queryLogStatus", json!(ALL_LOG_STATUSES[0]))],
        },
        UpdateCase {
            label: "jobLogStatus flipped to ENABLED",
            update: json!({ "jobLogStatus": ALL_LOG_STATUSES[0] }),
            expected: vec![("jobLogStatus", json!(ALL_LOG_STATUSES[0]))],
        },
        UpdateCase {
            label: "jobLogStatus restated as DISABLED",
            update: json!({ "jobLogStatus": ALL_LOG_STATUSES[1] }),
            expected: vec![("jobLogStatus", json!(ALL_LOG_STATUSES[1]))],
        },
        UpdateCase {
            label: "defaultResultConfiguration added as CSV with a key prefix",
            update: json!({
                "defaultResultConfiguration": query_result_configuration("CSV", Some("out")),
            }),
            expected: vec![(
                "defaultResultConfiguration",
                query_result_configuration("CSV", Some("out")),
            )],
        },
        UpdateCase {
            // The other `ResultFormat` value, and the optional `keyPrefix` left out.
            label: "defaultResultConfiguration added as PARQUET without a key prefix",
            update: json!({
                "defaultResultConfiguration": query_result_configuration("PARQUET", None),
            }),
            expected: vec![(
                "defaultResultConfiguration",
                query_result_configuration("PARQUET", None),
            )],
        },
        UpdateCase {
            label: "defaultJobResultConfiguration added",
            update: json!({
                "defaultJobResultConfiguration": job_result_configuration("job-results"),
            }),
            expected: vec![(
                "defaultJobResultConfiguration",
                job_result_configuration("job-results"),
            )],
        },
        UpdateCase {
            label: "every updatable member at once",
            update: json!({
                "queryLogStatus": ALL_LOG_STATUSES[1],
                "jobLogStatus": ALL_LOG_STATUSES[0],
                "defaultResultConfiguration": query_result_configuration("PARQUET", Some("all")),
                "defaultJobResultConfiguration": job_result_configuration("all-results"),
                "membershipPaymentConfiguration": {
                    "machineLearning": { "modelInference": { "isResponsible": true } },
                },
            }),
            expected: vec![
                ("queryLogStatus", json!(ALL_LOG_STATUSES[1])),
                ("jobLogStatus", json!(ALL_LOG_STATUSES[0])),
                (
                    "defaultResultConfiguration",
                    query_result_configuration("PARQUET", Some("all")),
                ),
                (
                    "defaultJobResultConfiguration",
                    job_result_configuration("all-results"),
                ),
                (
                    "paymentConfiguration",
                    json!({
                        "queryCompute": { "isResponsible": true },
                        "machineLearning": {
                            "modelTraining": { "isResponsible": false },
                            "modelInference": { "isResponsible": true },
                            "syntheticDataGeneration": { "isResponsible": false },
                        },
                        "jobCompute": { "isResponsible": false },
                    }),
                ),
            ],
        },
        UpdateCase {
            // A nested sub-structure: naming ONE ML cost leaves the other two as stored.
            label: "membershipPaymentConfiguration names one ML cost",
            update: json!({
                "membershipPaymentConfiguration": {
                    "machineLearning": { "syntheticDataGeneration": { "isResponsible": true } },
                },
            }),
            expected: vec![(
                "paymentConfiguration",
                json!({
                    "queryCompute": { "isResponsible": true },
                    "machineLearning": {
                        "modelTraining": { "isResponsible": false },
                        "modelInference": { "isResponsible": false },
                        "syntheticDataGeneration": { "isResponsible": true },
                    },
                    "jobCompute": { "isResponsible": false },
                }),
            )],
        },
        UpdateCase {
            // Restating the responsibilities the collaboration assigned is accepted and
            // changes nothing.
            label: "membershipPaymentConfiguration restates the assigned responsibilities",
            update: json!({
                "membershipPaymentConfiguration": {
                    "queryCompute": { "isResponsible": true },
                    "jobCompute": { "isResponsible": false },
                },
            }),
            expected: vec![("paymentConfiguration", baseline_payment_configuration())],
        },
    ];

    for case in cases {
        let label = case.label;
        let (membership_id, baseline) = seeded_membership(&emulator, label, &["CAN_QUERY"]).await;
        assert_eq!(
            baseline["paymentConfiguration"],
            baseline_payment_configuration(),
            "case {label:?}: the baseline membership's accepted payment configuration, got {}",
            baseline["paymentConfiguration"]
        );

        let (status, response) = update_membership(&emulator, &membership_id, case.update).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected UpdateMembership 200, got {status} with {response}"
        );
        let updated = response["membership"].clone();

        for member in MEMBERSHIP_MEMBERS {
            let expected = case
                .expected
                .iter()
                .find(|(name, _)| *name == member)
                .map(|(_, value)| value.clone());
            match expected {
                Some(expected) => assert_eq!(
                    updated[member], expected,
                    "case {label:?}: {member} must carry the updated value, got {:?}",
                    updated.get(member)
                ),
                None => assert_eq!(
                    updated.get(member),
                    baseline.get(member),
                    "case {label:?}: {member} was not in the request so it must be unchanged, got {:?}",
                    updated.get(member)
                ),
            }
        }

        let created_at = timestamp(&updated, "createTime", label);
        let updated_at = timestamp(&updated, "updateTime", label);
        assert!(
            updated_at >= created_at,
            "case {label:?}: updateTime must not precede createTime, got {updated_at} < {created_at}"
        );

        // Round trip: the reader returns exactly what the update persisted.
        let (status, fetched) = emulator.get_membership(&membership_id).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected GetMembership 200, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["membership"], updated,
            "case {label:?}: GetMembership must report the updated membership, got {}",
            fetched["membership"]
        );
    }
}

/// One row of the payment-responsibility table.
struct PaymentCase {
    label: &'static str,
    /// The creator's `memberAbilities`, which decide who pays.
    abilities: &'static [&'static str],
    /// The `membershipPaymentConfiguration` the update carries.
    requested: Value,
    /// Whether the collaboration assigns the caller exactly those responsibilities.
    accepted: bool,
}

#[tokio::test]
async fn update_membership_refuses_a_payment_responsibility_the_collaboration_did_not_assign() {
    let emulator = Emulator::new();

    // `MembershipQueryComputePaymentConfig$isResponsible` and
    // `MembershipJobComputePaymentConfig$isResponsible`: "An error message is returned
    // ... if you set the value to FALSE but you are responsible to pay ... [or] to TRUE
    // but you are not responsible to pay". Responsibility follows the paying ability
    // (CAN_QUERY for query compute, CAN_RUN_JOB for job compute), so the rows span both
    // costs, both requested booleans and the ability sets that grant neither, one or
    // both of them.
    let cases = [
        PaymentCase {
            label: "CAN_QUERY accepts query compute",
            abilities: &["CAN_QUERY"],
            requested: json!({ "queryCompute": { "isResponsible": true } }),
            accepted: true,
        },
        PaymentCase {
            label: "CAN_QUERY declines query compute",
            abilities: &["CAN_QUERY"],
            requested: json!({ "queryCompute": { "isResponsible": false } }),
            accepted: false,
        },
        PaymentCase {
            label: "CAN_QUERY declines job compute",
            abilities: &["CAN_QUERY"],
            requested: json!({ "jobCompute": { "isResponsible": false } }),
            accepted: true,
        },
        PaymentCase {
            label: "CAN_QUERY accepts job compute",
            abilities: &["CAN_QUERY"],
            requested: json!({ "jobCompute": { "isResponsible": true } }),
            accepted: false,
        },
        PaymentCase {
            label: "CAN_RUN_JOB accepts job compute",
            abilities: &["CAN_RUN_JOB"],
            requested: json!({ "jobCompute": { "isResponsible": true } }),
            accepted: true,
        },
        PaymentCase {
            label: "CAN_RUN_JOB accepts query compute",
            abilities: &["CAN_RUN_JOB"],
            requested: json!({ "queryCompute": { "isResponsible": true } }),
            accepted: false,
        },
        PaymentCase {
            label: "CAN_QUERY and CAN_RUN_JOB accept both costs",
            abilities: &["CAN_QUERY", "CAN_RUN_JOB"],
            requested: json!({
                "queryCompute": { "isResponsible": true },
                "jobCompute": { "isResponsible": true },
            }),
            accepted: true,
        },
        PaymentCase {
            label: "a member with neither paying ability declines both costs",
            abilities: &["CAN_RECEIVE_RESULTS"],
            requested: json!({
                "queryCompute": { "isResponsible": false },
                "jobCompute": { "isResponsible": false },
            }),
            accepted: true,
        },
        PaymentCase {
            label: "a member with neither paying ability accepts query compute",
            abilities: &["CAN_RECEIVE_RESULTS"],
            requested: json!({ "queryCompute": { "isResponsible": true } }),
            accepted: false,
        },
        PaymentCase {
            label: "an ML cost carries no ability, so either answer is accepted",
            abilities: &["CAN_RECEIVE_RESULTS"],
            requested: json!({
                "machineLearning": { "modelTraining": { "isResponsible": true } },
            }),
            accepted: true,
        },
    ];

    for case in cases {
        let label = case.label;
        let (membership_id, baseline) = seeded_membership(&emulator, label, case.abilities).await;
        let (status, response) = update_membership(
            &emulator,
            &membership_id,
            json!({ "membershipPaymentConfiguration": case.requested.clone() }),
        )
        .await;

        if case.accepted {
            assert_eq!(
                status, 200,
                "case {label:?}: the responsibility matches the collaboration, so expected 200, got {status} with {response}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for a responsibility the collaboration did not assign, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {response}"
        );

        // A refused update persists nothing.
        let (_, fetched) = emulator.get_membership(&membership_id).await;
        assert_eq!(
            fetched["membership"], baseline,
            "case {label:?}: a refused update must leave the membership untouched, got {}",
            fetched["membership"]
        );
    }
}

#[tokio::test]
async fn update_membership_rejects_a_membership_that_is_not_there() {
    let emulator = Emulator::new();

    // Both rows carry an identifier the model's `MembershipIdentifier` pattern accepts
    // (36 lowercase-hex characters), so the 404 comes from the handler's state lookup
    // rather than from constraint validation: one id that never existed, and one that
    // existed and was deleted.
    let (deleted_id, _) = seeded_membership(&emulator, "deleted", &["CAN_QUERY"]).await;
    let (status, body) = emulator
        .call("DELETE", &format!("/memberships/{deleted_id}"), None)
        .await;
    assert_eq!(
        status, 204,
        "DeleteMembership must succeed before the update is retried, got {status} with {body}"
    );

    for (label, id) in [
        ("never created", "00000000-0000-4000-8000-000000000000".to_string()),
        ("deleted", deleted_id),
    ] {
        let (status, response) =
            update_membership(&emulator, &id, json!({ "queryLogStatus": "ENABLED" })).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 for membership {id}, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            response["resourceType"].as_str(),
            Some("MEMBERSHIP"),
            "case {label:?}: the error names the missing resource type, got {response}"
        );
    }
}

#[tokio::test]
async fn update_membership_conflicts_once_the_membership_is_no_longer_active() {
    let emulator = Emulator::new();

    let label = "collaboration deleted";
    let (membership_id, baseline) = seeded_membership(&emulator, label, &["CAN_QUERY"]).await;
    let collaboration_id = baseline["collaborationId"].as_str().expect("collaborationId");
    let (status, body) = emulator
        .call("DELETE", &format!("/collaborations/{collaboration_id}"), None)
        .await;
    assert_eq!(
        status, 204,
        "case {label:?}: DeleteCollaboration must succeed, got {status} with {body}"
    );

    let (_, fetched) = emulator.get_membership(&membership_id).await;
    assert_eq!(
        fetched["membership"]["status"].as_str(),
        Some("COLLABORATION_DELETED"),
        "case {label:?}: deleting the collaboration leaves the membership COLLABORATION_DELETED, got {}",
        fetched["membership"]["status"]
    );
    let before = fetched["membership"].clone();

    let (status, response) = update_membership(
        &emulator,
        &membership_id,
        json!({ "queryLogStatus": "DISABLED" }),
    )
    .await;
    assert_eq!(
        status, 409,
        "case {label:?}: expected 409 for a membership that is not ACTIVE, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ConflictException",
        "case {label:?}: expected ConflictException, got {response}"
    );
    assert_eq!(
        response["reason"].as_str(),
        Some("INVALID_STATE"),
        "case {label:?}: the conflict reason is INVALID_STATE, got {response}"
    );

    let (_, fetched) = emulator.get_membership(&membership_id).await;
    assert_eq!(
        fetched["membership"], before,
        "case {label:?}: the refused update must persist nothing, got {}",
        fetched["membership"]
    );
}
