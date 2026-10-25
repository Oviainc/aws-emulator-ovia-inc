// SPDX-License-Identifier: CC0-1.0

//! How `UpdateSchedulingPolicy` WORDS its refusals, end to end over the wire.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * an identifier of the right shape that names no stored scheduling policy is refused
//!   with the modelled `ClientException` (HTTP 400) whose message is the ARN the
//!   identifier resolved to followed by ` does not exist`, TERMINATED with a period — the
//!   sentence the scheduling-policy resolver closes, built from the request and carrying
//!   nothing else (no frontend envelope, no resource-kind prose);
//! * an update that would change a policy's KIND is refused with the modelled
//!   `ClientException` (HTTP 400) naming the direction that was refused — the kind the
//!   STORED policy carries, the kind the request asked for, and the remediation — and it
//!   leaves the stored policy exactly as it was;
//! * a policy stored with NEITHER kind has no kind to change, so an update that sets
//!   either kind is accepted: the kind guard refuses a CHANGE, not a first write.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, error_code, error_message, Batch};

/// `CreateSchedulingPolicy`, answering the ARN it minted.
async fn create(batch: &Batch, body: Value) -> String {
    let created = batch.ok("/v1/createschedulingpolicy", body).await;
    created["arn"].as_str().unwrap_or_default().to_string()
}

/// `UpdateSchedulingPolicy` over the wire: the status and the decoded body.
async fn update(batch: &Batch, body: Value) -> (u16, Value) {
    batch.call("/v1/updateschedulingpolicy", body).await
}

/// The one `SchedulingPolicyDetail` stored under `policy_arn`.
async fn describe(batch: &Batch, policy_arn: &str) -> Value {
    let body = batch
        .ok(
            "/v1/describeschedulingpolicies",
            json!({"arns": [policy_arn]}),
        )
        .await;
    body["schedulingPolicies"][0].clone()
}

/// RULE: an identifier of the right shape naming no stored scheduling policy is refused
/// 400 / `ClientException` with `<the ARN the identifier resolved to> does not exist.` —
/// the ARN built from the request, the sentence terminated with a period.
#[tokio::test]
async fn an_absent_scheduling_policy_is_refused_with_the_terminated_resolved_arn_sentence() {
    let batch = Batch::new();

    // The rows vary what the rule generalises over: the name's length and alphabet, the
    // identifier FORM (the name, and the full ARN it resolves to), and which policy member
    // the update carries — the resolution happens before any of them is applied.
    let names: Vec<String> = vec![
        "sp-absent".to_string(),
        "SP_Absent-2".to_string(),
        "s".to_string(),
        "a".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN),
    ];
    let policies: Vec<(&str, Option<Value>)> = vec![
        ("no policy member at all", None),
        (
            "a fairsharePolicy",
            Some(json!({"fairsharePolicy": {"computeReservation": 5, "shareDecaySeconds": 60}})),
        ),
        (
            "a quotaSharePolicy",
            Some(json!({"quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"}})),
        ),
    ];

    for name in &names {
        let resolved = arn(&format!("scheduling-policy/{name}"));
        for (form, identifier) in [("name", name.clone()), ("arn", resolved.clone())] {
            for (label, carried) in &policies {
                let case = format!("{label} by {form} {identifier}");
                let mut body = json!({"arn": identifier});
                if let Some(members) = carried.as_ref().and_then(Value::as_object) {
                    for (member, value) in members {
                        body[member.as_str()] = value.clone();
                    }
                }
                let (status, refused) = update(&batch, body).await;
                assert_eq!(
                    status, 400,
                    "case {case}: expected the modelled ClientException 400, got {status} \
                     {refused}"
                );
                assert_eq!(
                    error_code(&refused),
                    "ClientException",
                    "case {case}: expected the modelled ClientException, got {refused}"
                );
                assert_eq!(
                    error_message(&refused),
                    format!("{resolved} does not exist."),
                    "case {case}: expected the resolved-ARN sentence terminated with a \
                     period, got {refused}"
                );
            }
        }
    }
}

/// One refused kind change: the policy as it is CREATED, and the member the update then
/// carries — the other kind than the one stored.
struct KindChange {
    /// What this row varies; named in every assertion.
    label: &'static str,
    name: &'static str,
    /// The `fairsharePolicy`/`quotaSharePolicy` member the create carries.
    created_with: Value,
    /// The kind the stored policy therefore has, as AWS names it in the refusal.
    stored_kind: &'static str,
    /// The member the update carries, which is the OTHER kind.
    requested: Value,
    /// The kind the update asked for, as AWS names it in the refusal.
    requested_kind: &'static str,
}

/// The table: both DIRECTIONS of a kind change, each over policies of a different shape —
/// an empty policy of its kind, one carrying every member it documents, and one carrying a
/// share distribution — so the refusal is shown to be built from the kinds and never from
/// the policy's contents.
fn kind_changes() -> Vec<KindChange> {
    vec![
        KindChange {
            label: "an empty fairsharePolicy asked for a quotaSharePolicy",
            name: "sp-kind-fair-empty",
            created_with: json!({"fairsharePolicy": {}}),
            stored_kind: state::FAIRSHARE_POLICY_KIND,
            requested: json!({"quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"}}),
            requested_kind: state::QUOTA_SHARE_POLICY_KIND,
        },
        KindChange {
            label: "a fairsharePolicy with decay and reservation asked for a quotaSharePolicy",
            name: "sp_kind_fair_full",
            created_with: json!({
                "fairsharePolicy": {"shareDecaySeconds": 7200, "computeReservation": 2},
            }),
            stored_kind: state::FAIRSHARE_POLICY_KIND,
            requested: json!({"quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"}}),
            requested_kind: state::QUOTA_SHARE_POLICY_KIND,
        },
        KindChange {
            label: "a fairsharePolicy with a share distribution asked for a quotaSharePolicy",
            name: "SP-Kind-Fair-Shares",
            created_with: json!({
                "fairsharePolicy": {
                    "shareDistribution": [
                        {"shareIdentifier": "teamA", "weightFactor": 2.0},
                        {"shareIdentifier": "teamB", "weightFactor": 1.0},
                    ],
                },
            }),
            stored_kind: state::FAIRSHARE_POLICY_KIND,
            requested: json!({"quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"}}),
            requested_kind: state::QUOTA_SHARE_POLICY_KIND,
        },
        KindChange {
            label: "a quotaSharePolicy asked for an empty fairsharePolicy",
            name: "sp-kind-quota-empty",
            created_with: json!({
                "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"},
            }),
            stored_kind: state::QUOTA_SHARE_POLICY_KIND,
            requested: json!({"fairsharePolicy": {}}),
            requested_kind: state::FAIRSHARE_POLICY_KIND,
        },
        KindChange {
            label: "a quotaSharePolicy asked for a fairsharePolicy with members",
            name: "sp-kind-quota-full",
            created_with: json!({
                "quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"},
            }),
            stored_kind: state::QUOTA_SHARE_POLICY_KIND,
            requested: json!({
                "fairsharePolicy": {"shareDecaySeconds": 60, "computeReservation": 1},
            }),
            requested_kind: state::FAIRSHARE_POLICY_KIND,
        },
    ]
}

/// RULE: an update that would change a scheduling policy's kind is refused 400 /
/// `ClientException` with the direction it refused — the stored kind, the requested kind
/// and the remediation — and the stored policy is left exactly as it was.
#[tokio::test]
async fn changing_a_scheduling_policy_kind_is_refused_naming_both_kinds() {
    let batch = Batch::new();

    for row in kind_changes() {
        let case = row.label;
        let mut create_body = json!({"name": row.name});
        for (member, value) in row.created_with.as_object().into_iter().flatten() {
            create_body[member.as_str()] = value.clone();
        }
        let policy_arn = create(&batch, create_body).await;
        let before = describe(&batch, &policy_arn).await;

        let mut body = json!({"arn": &policy_arn});
        for (member, value) in row.requested.as_object().into_iter().flatten() {
            body[member.as_str()] = value.clone();
        }
        let (status, refused) = update(&batch, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case}: expected the modelled ClientException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!(
                "Cannot change scheduling policy type from {} to {}. Create a new scheduling \
                 policy instead.",
                row.stored_kind, row.requested_kind
            ),
            "case {case}: expected the refusal to name the direction it refused, got {refused}"
        );
        assert_eq!(
            describe(&batch, &policy_arn).await,
            before,
            "case {case}: a refused kind change must leave the stored policy as it was"
        );
    }
}

/// RULE: the kind guard refuses a CHANGE of kind, not a first write — a policy stored with
/// neither kind takes either one, and the write is readable afterwards.
#[tokio::test]
async fn a_policy_stored_with_no_kind_takes_either_kind() {
    let batch = Batch::new();

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a fairsharePolicy on a policy with no kind",
            "sp-kindless-fair",
            json!({"fairsharePolicy": {"computeReservation": 3, "shareDecaySeconds": 120}}),
        ),
        (
            "a quotaSharePolicy on a policy with no kind",
            "sp-kindless-quota",
            json!({"quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"}}),
        ),
    ];

    for (case, name, requested) in rows {
        let policy_arn = create(&batch, json!({"name": name})).await;
        let mut body = json!({"arn": &policy_arn});
        for (member, value) in requested.as_object().into_iter().flatten() {
            body[member.as_str()] = value.clone();
        }
        let (status, answered) = update(&batch, body).await;
        assert_eq!(
            status, 200,
            "case {case}: a first write of a kind is accepted, got {status} {answered}"
        );

        // Read the write back through the implemented reader: the kind the update set is
        // the kind the policy now carries.
        let detail = describe(&batch, &policy_arn).await;
        if requested.get("quotaSharePolicy").is_some() {
            assert_eq!(
                detail["quotaSharePolicy"],
                requested["quotaSharePolicy"],
                "case {case}: expected the quotaSharePolicy the update set, got {detail}"
            );
        } else {
            assert_eq!(
                detail["fairsharePolicy"]["computeReservation"],
                requested["fairsharePolicy"]["computeReservation"],
                "case {case}: expected the fairsharePolicy the update set, got {detail}"
            );
            assert_eq!(
                detail["fairsharePolicy"]["shareDecaySeconds"],
                requested["fairsharePolicy"]["shareDecaySeconds"],
                "case {case}: expected the fairsharePolicy the update set, got {detail}"
            );
        }
    }
}
