// SPDX-License-Identifier: CC0-1.0

//! `UpdateSchedulingPolicy` end to end over the wire, read back through
//! `DescribeSchedulingPolicies`.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * an update stores the policy member the request carried WHOLE — the stored member is
//!   replaced by what was submitted rather than merged into what was there — so the detail
//!   a later read answers is the one the last accepted update left behind, whichever kind
//!   the policy carries and however many entries its distribution has;
//! * the policy is named in either shape a scheduling-policy identifier has: the ARN the
//!   reader reports, or the name that ARN carries;
//! * an update mints no new identity and touches nothing beside the policy member: the
//!   name, the ARN and the tag map stay the create's;
//! * an update that carries neither policy member changes nothing, and answers the same
//!   modelled empty 200;
//! * a describe is a REPORT, so two reads at the same instant answer identically.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, Batch};
use crate::scheduling_policies::materialized_fairshare;

/// Which shape of identifier the row names its policy by. Both are accepted by the
/// writers of this service (`state::is_scheduling_policy_reference`).
#[derive(Debug, Clone, Copy)]
enum Reference {
    /// The ARN `DescribeSchedulingPolicies` reports.
    Arn,
    /// The `<name>` inside that ARN.
    Name,
}

/// One policy, and the updates applied to it in order.
#[derive(Debug, Clone)]
struct Row {
    /// What the row varies: named in every assertion.
    label: String,
    name: String,
    /// The create request's members beside `name`.
    create: Value,
    /// How each update of this row names the policy.
    reference: Reference,
    /// The policy members each successive update carries.
    updates: Vec<Value>,
}

/// The `schedulingPolicies` `DescribeSchedulingPolicies` answers for `arns`, requiring a
/// 200.
async fn describe(batch: &Batch, arns: &[&str]) -> Vec<Value> {
    let body = batch
        .ok("/v1/describeschedulingpolicies", json!({"arns": arns}))
        .await;
    body["schedulingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The one `SchedulingPolicyDetail` stored for `policy_arn`.
async fn describe_one(batch: &Batch, case: &str, policy_arn: &str) -> Value {
    let found = describe(batch, &[policy_arn]).await;
    assert_eq!(
        found.len(),
        1,
        "case {case}: expected exactly one schedulingPolicy for {policy_arn:?}, got {found:?}"
    );
    found[0].clone()
}

/// Create the policy `body` asks for; answers its ARN.
async fn create(batch: &Batch, case: &str, body: Value) -> String {
    let created = batch.ok("/v1/createschedulingpolicy", body).await;
    let policy_arn = created["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !policy_arn.is_empty(),
        "case {case}: expected a minted ARN, got {created}"
    );
    policy_arn
}

/// `UpdateSchedulingPolicy` for `reference` carrying `members`; answers the status and
/// the decoded body.
async fn update(batch: &Batch, reference: &str, members: &Value) -> (u16, Value) {
    let mut body = json!({"arn": reference});
    if let Some(object) = members.as_object() {
        for (member, value) in object {
            body[member.as_str()] = value.clone();
        }
    }
    batch.call("/v1/updateschedulingpolicy", body).await
}

/// `count` distinct, non-overlapping `shareDistribution` entries starting at `first`,
/// each carrying a dyadic `weightFactor` so the read-back comparison is exact.
fn shares(first: usize, count: usize) -> Value {
    Value::Array(
        (first..first + count)
            .map(|index| {
                json!({
                    "shareIdentifier": format!("u{index}"),
                    "weightFactor": 0.5 + index as f64,
                })
            })
            .collect(),
    )
}

/// A `quotaSharePolicy` carrying the `idleResourceAssignmentStrategy` at `index` of every
/// value the model declares, wrapping round at the end. Driven by
/// `state::QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES` (the model's
/// `QuotaShareIdleResourceAssignmentStrategy` enum) rather than by a literal, so the rows
/// span the whole value set however many values it gains.
fn quota_share(index: usize) -> Value {
    let strategies = state::QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES;
    json!({"idleResourceAssignmentStrategy": strategies[index % strategies.len()]})
}

/// One update per `idleResourceAssignmentStrategy` the model declares, applied in order.
fn quota_share_updates() -> Vec<Value> {
    (0..state::QUOTA_SHARE_IDLE_RESOURCE_ASSIGNMENT_STRATEGIES.len())
        .map(|index| json!({"quotaSharePolicy": quota_share(index)}))
        .collect()
}

/// The table: rows differ along the kind the policy was created with, which shape names
/// it, which members each update carries, how many distribution entries it has and in
/// which order, and how many updates are applied in a row.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a policy created with neither kind, given a fair-share policy by ARN".to_string(),
            name: "sp-up-bare-to-fair".to_string(),
            create: json!({}),
            reference: Reference::Arn,
            updates: vec![json!({
                "fairsharePolicy": {"shareDecaySeconds": 600, "computeReservation": 25},
            })],
        },
        Row {
            label: "a policy created with neither kind, given a quota-share policy by name".to_string(),
            name: "sp_up_bare_to_quota".to_string(),
            create: json!({"tags": {"team": "research"}}),
            reference: Reference::Name,
            updates: quota_share_updates(),
        },
        Row {
            label: "a fair-share policy replaced wholesale, shrinking to nothing, by name".to_string(),
            name: "SP-Up-Replace-1".to_string(),
            create: json!({
                "fairsharePolicy": {
                    "shareDecaySeconds": 3600,
                    "computeReservation": 99,
                    "shareDistribution": shares(0, 3),
                },
                "tags": {"cost-centre": "42"},
            }),
            reference: Reference::Name,
            updates: vec![
                json!({"fairsharePolicy": {"shareDistribution": shares(7, 1)}}),
                json!({"fairsharePolicy": {"shareDecaySeconds": 1}}),
                json!({"fairsharePolicy": {}}),
            ],
        },
        Row {
            label: "a fair-share distribution that grows, reorders and shrinks, by ARN".to_string(),
            name: "sp-up-distribution".to_string(),
            create: json!({"fairsharePolicy": {"shareDistribution": shares(0, 2)}}),
            reference: Reference::Arn,
            updates: vec![
                json!({"fairsharePolicy": {"shareDistribution": shares(0, 4)}}),
                json!({"fairsharePolicy": {
                    "shareDistribution": Value::Array(
                        shares(0, 4).as_array().cloned().unwrap_or_default().into_iter().rev().collect(),
                    ),
                }}),
                json!({"fairsharePolicy": {"shareDistribution": []}}),
            ],
        },
        Row {
            label: "a quota-share policy updated through every declared strategy, by ARN"
                .to_string(),
            name: "sp-up-quota".to_string(),
            create: json!({"quotaSharePolicy": quota_share(0)}),
            reference: Reference::Arn,
            // Every declared strategy, then the first one again: a repeated update is the
            // same accepted write.
            updates: quota_share_updates()
                .into_iter()
                .chain(std::iter::once(json!({"quotaSharePolicy": quota_share(0)})))
                .collect(),
        },
        Row {
            // The longest name the member documents, resolved by that NAME: the bound is
            // `state::MAX_SCHEDULING_POLICY_NAME_LEN`.
            label: "the longest accepted name, updated by that name".to_string(),
            name: "u".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN),
            create: json!({"fairsharePolicy": {"computeReservation": 1}}),
            reference: Reference::Name,
            updates: vec![json!({
                "fairsharePolicy": {
                    "shareDecaySeconds": state::MAX_SHARE_DECAY_SECONDS,
                    "computeReservation": state::MAX_COMPUTE_RESERVATION,
                    "shareDistribution": shares(2, 2),
                },
            })],
        },
    ]
}

/// The create body for `row`.
fn create_body(row: &Row) -> Value {
    let mut body = json!({"name": row.name});
    if let Some(object) = row.create.as_object() {
        for (member, value) in object {
            body[member.as_str()] = value.clone();
        }
    }
    body
}

#[tokio::test]
async fn an_update_replaces_the_policy_member_every_reader_reads() {
    let batch = Batch::new();

    // A policy no row updates, so each update is shown to write ITS policy and no other.
    let keeper_arn = create(
        &batch,
        "the untouched policy",
        json!({
            "name": "sp-up-keeper",
            "fairsharePolicy": {"shareDecaySeconds": 42, "shareDistribution": shares(5, 2)},
            "tags": {"keep": "me"},
        }),
    )
    .await;
    let keeper_before = describe_one(&batch, "the untouched policy", &keeper_arn).await;

    for row in rows() {
        let label = &row.label;
        let policy_arn = create(&batch, label, create_body(&row)).await;
        assert_eq!(
            policy_arn,
            arn(&format!("scheduling-policy/{}", row.name)),
            "case {label}: expected the ARN minted for the created name, got {policy_arn:?}"
        );
        let reference = match row.reference {
            Reference::Arn => policy_arn.clone(),
            Reference::Name => row.name.clone(),
        };
        let created = describe_one(&batch, label, &policy_arn).await;

        // What the STORE holds, tracked the way the handler writes it: an update replaces
        // the member it carries and leaves the other kind alone.
        let mut fairshare = row.create.get("fairsharePolicy").cloned();
        let mut quota_share = row.create.get("quotaSharePolicy").cloned();

        for (attempt, members) in row.updates.iter().enumerate() {
            let case = format!("{label} (update {})", attempt + 1);
            let (status, answered) = update(&batch, &reference, members).await;
            assert_eq!(
                status, 200,
                "case {case}: expected the empty 200, got {status} {answered}"
            );
            assert!(
                answered.is_null() || answered == json!({}),
                "case {case}: expected an empty output, got {answered}"
            );

            if let Some(submitted) = members.get("fairsharePolicy") {
                fairshare = Some(submitted.clone());
            }
            if let Some(submitted) = members.get("quotaSharePolicy") {
                quota_share = Some(submitted.clone());
            }

            let detail = describe_one(&batch, &case, &policy_arn).await;
            assert_eq!(
                detail["fairsharePolicy"],
                materialized_fairshare(fairshare.as_ref(), quota_share.as_ref()),
                "case {case}: expected the submitted fairsharePolicy stored whole, got {detail}"
            );
            assert_eq!(
                detail["quotaSharePolicy"],
                quota_share.clone().unwrap_or(Value::Null),
                "case {case}: expected the submitted quotaSharePolicy stored whole, got {detail}"
            );

            // An update mints no new identity and touches nothing beside the policy.
            for member in ["name", "arn", "tags"] {
                assert_eq!(
                    detail[member], created[member],
                    "case {case}: an update must leave {member} as the create left it, got \
                     {detail}"
                );
            }

            // A describe reports; it does not write what the next one sees.
            let again = describe_one(&batch, &case, &policy_arn).await;
            assert_eq!(
                again, detail,
                "case {case}: two reads at the same instant must answer identically, got \
                 {again} then {detail}"
            );
        }

        assert_eq!(
            describe_one(&batch, label, &keeper_arn).await,
            keeper_before,
            "case {label}: an update must leave every other policy untouched"
        );
    }
}

#[tokio::test]
async fn an_update_carrying_neither_policy_member_changes_nothing() {
    let batch = Batch::new();

    for row in rows() {
        let label = &row.label;
        let policy_arn = create(&batch, label, create_body(&row)).await;
        let before = describe_one(&batch, label, &policy_arn).await;

        // Both identifier shapes, and the call repeated: a request that asks for no
        // change is the state it asks for, so it is the same empty success every time.
        for reference in [policy_arn.clone(), row.name.clone()] {
            for attempt in 1..=2 {
                let case = format!("{label}: no member, by {reference:?} (attempt {attempt})");
                let (status, answered) = update(&batch, &reference, &json!({})).await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected the empty 200, got {status} {answered}"
                );
                assert!(
                    answered.is_null() || answered == json!({}),
                    "case {case}: expected an empty output, got {answered}"
                );
                assert_eq!(
                    describe_one(&batch, &case, &policy_arn).await,
                    before,
                    "case {case}: an update carrying neither member must change nothing"
                );
            }
        }
    }
}
