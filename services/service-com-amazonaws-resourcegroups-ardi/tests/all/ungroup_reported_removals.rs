// SPDX-License-Identifier: CC0-1.0

//! What `UngroupResources` REPORTS it did, end to end.
//!
//! The rule under test, asserted over a TABLE of requests rather than one row: the
//! per-resource result lists report what the call DID to each named resource, not the
//! state it left behind. So `Succeeded` holds exactly the memberships this call
//! dropped — read back as the difference between the group's listing before and
//! after, never from the request — and a named resource the group does not hold is
//! removed by nothing, recorded against nothing and reported in neither `Succeeded`
//! nor `Failed`.
//!
//! Rows differ along what the rule generalises over: how many named resources the
//! group holds (all, some, none), whether a named resource exists in another group
//! instead, repetition inside one request, and the size of the request up to the
//! modelled maximum of ten ARNs.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::resource_group_lifecycle::Emulator;
use super::ungroup_resources::{group_with_members, listed_members, sorted, succeeded, RESOURCES};

/// Resource ARNs no group in this suite is ever grouped with.
const STRANGERS: [&str; 3] = [
    "arn:aws:ec2:us-east-1:000000000000:volume/vol-00000000000000000",
    "arn:aws:s3:::never-grouped-bucket",
    "arn:aws:sqs:us-east-1:000000000000:never-grouped-queue",
];

/// One row: the memberships the addressed group is created with, the resources one
/// request names, and the memberships a BYSTANDER group is created with — the
/// resources that exist in the account but not in the addressed group.
struct Row {
    label: &'static str,
    grouped: Vec<String>,
    requested: Vec<String>,
    elsewhere: Vec<String>,
}

/// `arns` as owned strings, in the order given.
fn owned<const N: usize>(arns: [&str; N]) -> Vec<String> {
    arns.iter().map(|arn| (*arn).to_string()).collect()
}

/// Every shape of request the rule covers.
fn rows() -> Vec<Row> {
    // Ten distinct ARNs: the most the modelled `ResourceArnList` (`length 1..10`)
    // carries, so a row can span a full request.
    let ten: Vec<String> = (0..10)
        .map(|i| format!("arn:aws:ec2:us-east-1:000000000000:instance/i-{i:017x}"))
        .collect();
    vec![
        Row {
            label: "only resources the group never held",
            grouped: owned([RESOURCES[0], RESOURCES[1]]),
            requested: owned(STRANGERS),
            elsewhere: Vec::new(),
        },
        Row {
            label: "one member beside two resources the group never held",
            grouped: owned([RESOURCES[0], RESOURCES[1]]),
            requested: vec![
                RESOURCES[0].to_string(),
                STRANGERS[0].to_string(),
                STRANGERS[1].to_string(),
            ],
            elsewhere: Vec::new(),
        },
        Row {
            label: "a resource that is a member of another group only",
            grouped: owned([RESOURCES[0]]),
            requested: owned([RESOURCES[0], RESOURCES[2], RESOURCES[3]]),
            elsewhere: owned([RESOURCES[2], RESOURCES[3]]),
        },
        Row {
            label: "every member the group holds",
            grouped: owned([RESOURCES[0], RESOURCES[1], RESOURCES[2]]),
            requested: owned([RESOURCES[0], RESOURCES[1], RESOURCES[2]]),
            elsewhere: Vec::new(),
        },
        Row {
            label: "a member named twice beside a stranger named twice",
            grouped: owned([RESOURCES[0], RESOURCES[1]]),
            requested: vec![
                RESOURCES[0].to_string(),
                STRANGERS[0].to_string(),
                RESOURCES[0].to_string(),
                STRANGERS[0].to_string(),
            ],
            elsewhere: Vec::new(),
        },
        Row {
            label: "ten ARNs, half of them members",
            grouped: ten[..5].to_vec(),
            requested: ten.clone(),
            elsewhere: ten[5..].to_vec(),
        },
        Row {
            label: "a single member, the smallest request",
            grouped: owned([RESOURCES[4]]),
            requested: owned([RESOURCES[4]]),
            elsewhere: Vec::new(),
        },
    ]
}

/// The `Failed` and `Pending` lists of an answer must both be empty: a removal this
/// emulator accepts is applied in full and immediately, and a resource it removed
/// nothing for did not FAIL to be removed.
fn assert_no_failed_or_pending(case: &str, answered: &Value) {
    for empty in ["Failed", "Pending"] {
        let list = answered[empty].as_array();
        assert!(
            list.is_none_or(Vec::is_empty),
            "case {case:?}: expected no {empty} entry, got {answered}",
        );
    }
}

/// The resources `ListGroupingStatuses` holds a record for, in `group`.
async fn recorded_resources(emulator: &Emulator, case: &str, group: &str) -> Vec<String> {
    let page = emulator
        .ok("/list-grouping-statuses", &json!({ "Group": group }))
        .await;
    sorted(
        page["GroupingStatuses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected GroupingStatuses, got {page}"))
            .iter()
            .map(|item| {
                item["ResourceArn"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: a record must carry ResourceArn, got {item}")
                    })
                    .to_string()
            }),
    )
}

/// `Succeeded` reports exactly the memberships the call dropped — the difference
/// between the group's listing before and after — and a named resource the group did
/// not hold is reported in nothing and recorded against nothing.
#[tokio::test]
async fn succeeded_reports_exactly_the_memberships_the_call_dropped() {
    for row in rows() {
        let label = row.label;
        let emulator = Emulator::new();
        group_with_members(&emulator, "addressed", &row.grouped).await;
        if !row.elsewhere.is_empty() {
            group_with_members(&emulator, "bystander", &row.elsewhere).await;
        }

        let before = listed_members(&emulator, label, "addressed").await;
        let (status, answered) = emulator
            .call(
                "/ungroup-resources",
                &json!({ "Group": "addressed", "ResourceArns": row.requested }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {answered}",
        );
        let after = listed_members(&emulator, label, "addressed").await;
        let dropped: Vec<String> = before
            .iter()
            .filter(|arn| !after.contains(arn))
            .cloned()
            .collect();
        let reported = succeeded(label, &answered);
        assert_eq!(
            reported, dropped,
            "case {label:?}: expected Succeeded to be the memberships the group LOST, \
             {dropped:?}, got {reported:?} ({answered})",
        );
        assert_no_failed_or_pending(label, &answered);

        // No named resource the group did not hold is reported anywhere, and the
        // call recorded no grouping action for it either: nothing was performed on
        // it, so nothing claims it was.
        let held: BTreeSet<&String> = before.iter().collect();
        let recorded = recorded_resources(&emulator, label, "addressed").await;
        for arn in row.requested.iter().filter(|arn| !held.contains(arn)) {
            assert!(
                !reported.contains(arn),
                "case {label:?}: {arn} was never a membership, so no removal of it \
                 succeeded, got {answered}",
            );
            assert!(
                !recorded.contains(arn),
                "case {label:?}: {arn} was never a membership, so the call recorded no \
                 action against it, got {recorded:?}",
            );
        }

        // A resource that lives in another group keeps its membership there: a
        // removal reports and touches one group's memberships only.
        if !row.elsewhere.is_empty() {
            let bystander = listed_members(&emulator, label, "bystander").await;
            assert_eq!(
                bystander,
                sorted(&row.elsewhere),
                "case {label:?}: expected another group's memberships to be untouched, \
                 got {bystander:?}",
            );
        }

        // Repeating the request removes nothing left to remove, so it reports no
        // removal at all and leaves the same group behind.
        let (status, again) = emulator
            .call(
                "/ungroup-resources",
                &json!({ "Group": "addressed", "ResourceArns": row.requested }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the repeated call to be served, got {status} {again}",
        );
        let repeated = succeeded(label, &again);
        assert!(
            repeated.is_empty(),
            "case {label:?}: a repeated call removes nothing, so it reports no ARN, \
             got {repeated:?} ({again})",
        );
        assert_no_failed_or_pending(label, &again);
        let unchanged = listed_members(&emulator, label, "addressed").await;
        assert_eq!(
            unchanged, after,
            "case {label:?}: expected the repeated call to leave the memberships {after:?}, \
             got {unchanged:?}",
        );
    }
}
