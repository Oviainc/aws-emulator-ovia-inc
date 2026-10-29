// SPDX-License-Identifier: CC0-1.0

//! What `GroupResources` REPORTS it did, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rule under test, asserted over a TABLE of requests rather than one row: the
//! per-resource result lists report what the call DID to each named resource, not
//! the state it left behind. So `Succeeded` holds exactly the memberships this call
//! added — read back as the difference between the group's listing before and
//! after, never from the request — and a named resource the group already held is
//! added by nothing and reported in neither `Succeeded` nor `Failed`. It is the
//! model's own criterion for the member: "the resources that this operation
//! successfully added to the group"
//! (`smithy_query(query="doc",shape="GroupResourcesOutput",member="Succeeded")`).
//!
//! Rows differ along what the rule generalises over: how many named resources the
//! group already holds (none, some, all), whether a named resource is a member of
//! another group instead, repetition inside one request, and the size of the
//! request up to the modelled maximum of ten ARNs.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::resource_group_lifecycle::Emulator;
use super::ungroup_resources::{group_with_members, listed_members, sorted, succeeded, RESOURCES};

/// Resource ARNs no group in this suite is ever grouped with before the call under
/// test, so each is a membership the call itself has to add.
const NEWCOMERS: [&str; 3] = [
    "arn:aws:ec2:us-east-1:000000000000:volume/vol-00000000000000000",
    "arn:aws:s3:::newly-grouped-bucket",
    "arn:aws:sqs:us-east-1:000000000000:newly-grouped-queue",
];

/// One row: the memberships the addressed group is created with, the resources one
/// request names, and the memberships a BYSTANDER group is created with — the
/// resources that exist in another group but not in the addressed one.
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
            label: "only resources the group does not hold yet",
            grouped: owned([RESOURCES[0], RESOURCES[1]]),
            requested: owned(NEWCOMERS),
            elsewhere: Vec::new(),
        },
        Row {
            label: "one member the group already holds, beside two it does not",
            grouped: owned([RESOURCES[0], RESOURCES[1]]),
            requested: vec![
                RESOURCES[0].to_string(),
                NEWCOMERS[0].to_string(),
                NEWCOMERS[1].to_string(),
            ],
            elsewhere: Vec::new(),
        },
        Row {
            label: "only resources the group already holds",
            grouped: owned([RESOURCES[0], RESOURCES[1], RESOURCES[2]]),
            requested: owned([RESOURCES[0], RESOURCES[1], RESOURCES[2]]),
            elsewhere: Vec::new(),
        },
        Row {
            label: "a resource that is a member of another group only",
            grouped: owned([RESOURCES[0]]),
            requested: owned([RESOURCES[0], RESOURCES[2], RESOURCES[3]]),
            elsewhere: owned([RESOURCES[2], RESOURCES[3]]),
        },
        Row {
            label: "a new resource named twice beside a member named twice",
            grouped: owned([RESOURCES[0], RESOURCES[1]]),
            requested: vec![
                NEWCOMERS[0].to_string(),
                RESOURCES[0].to_string(),
                NEWCOMERS[0].to_string(),
                RESOURCES[0].to_string(),
            ],
            elsewhere: Vec::new(),
        },
        Row {
            label: "ten ARNs, half of them already members",
            grouped: ten[..5].to_vec(),
            requested: ten.clone(),
            elsewhere: Vec::new(),
        },
        Row {
            label: "a single new resource, the smallest request",
            grouped: Vec::new(),
            requested: owned([RESOURCES[4]]),
            elsewhere: Vec::new(),
        },
        Row {
            label: "a single resource the group already holds",
            grouped: owned([RESOURCES[5]]),
            requested: owned([RESOURCES[5]]),
            elsewhere: Vec::new(),
        },
    ]
}

/// The `Failed` and `Pending` lists of an answer must both be empty: an addition
/// this emulator accepts is applied in full and immediately, and a resource it
/// added nothing for did not FAIL to be added.
fn assert_no_failed_or_pending(case: &str, answered: &Value) {
    for empty in ["Failed", "Pending"] {
        let list = answered[empty].as_array();
        assert!(
            list.is_none_or(Vec::is_empty),
            "case {case:?}: expected no {empty} entry, got {answered}",
        );
    }
}

/// `Succeeded` reports exactly the memberships the call added — the difference
/// between the group's listing before and after — so a named resource the group
/// already held is reported in nothing, and a repeated call reports no addition at
/// all.
#[tokio::test]
async fn succeeded_reports_exactly_the_memberships_the_call_added() {
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
                "/group-resources",
                &json!({ "Group": "addressed", "ResourceArns": row.requested }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} {answered}",
        );
        let after = listed_members(&emulator, label, "addressed").await;
        let gained: Vec<String> = after
            .iter()
            .filter(|arn| !before.contains(arn))
            .cloned()
            .collect();
        let reported = succeeded(label, &answered);
        assert_eq!(
            reported, gained,
            "case {label:?}: expected Succeeded to be the memberships the group GAINED, \
             {gained:?}, got {reported:?} ({answered})",
        );
        assert_no_failed_or_pending(label, &answered);

        // The memberships the group already held are still there, exactly once
        // each: an addition adds to a membership rather than replacing it, and
        // recording an ARN that is already a member duplicates nothing.
        let expected_after = sorted(before.iter().chain(row.requested.iter()));
        assert_eq!(
            after, expected_after,
            "case {label:?}: expected the group to hold {expected_after:?}, got {after:?}",
        );

        // No named resource the group ALREADY held is reported anywhere: this call
        // added no membership for it, so nothing claims it did.
        let held: BTreeSet<&String> = before.iter().collect();
        for arn in row.requested.iter().filter(|arn| held.contains(arn)) {
            assert!(
                !reported.contains(arn),
                "case {label:?}: {arn} was already a membership, so this call added no \
                 membership for it, got {answered}",
            );
        }

        // A group that holds the same resources is untouched: an addition reports
        // and writes one group's memberships only.
        if !row.elsewhere.is_empty() {
            let bystander = listed_members(&emulator, label, "bystander").await;
            assert_eq!(
                bystander,
                sorted(&row.elsewhere),
                "case {label:?}: expected another group's memberships to be untouched, \
                 got {bystander:?}",
            );
        }

        // Repeating the request adds nothing left to add, so it reports no addition
        // at all and leaves the same group behind.
        let (status, again) = emulator
            .call(
                "/group-resources",
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
            "case {label:?}: a repeated call adds nothing, so it reports no ARN, \
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

/// An addition that happened and one that was a no-op are both the resource's last
/// grouping action — `GROUP`, `SUCCESS` — so a repeated call reports one record per
/// resource the group has held and never a second record for the same resource.
#[tokio::test]
async fn a_no_op_addition_still_leaves_one_grouping_record_per_resource() {
    let members = owned([RESOURCES[0], RESOURCES[1]]);
    let emulator = Emulator::new();
    group_with_members(&emulator, "records", &members).await;
    let mut requested = members.clone();
    requested.push(NEWCOMERS[0].to_string());
    emulator
        .ok(
            "/group-resources",
            &json!({ "Group": "records", "ResourceArns": requested }),
        )
        .await;

    let page = emulator
        .ok("/list-grouping-statuses", &json!({ "Group": "records" }))
        .await;
    let items = page["GroupingStatuses"]
        .as_array()
        .unwrap_or_else(|| panic!("expected GroupingStatuses, got {page}"))
        .clone();
    let listed = sorted(items.iter().map(|item| {
        item["ResourceArn"]
            .as_str()
            .unwrap_or_else(|| panic!("a record must carry ResourceArn, got {item}"))
            .to_string()
    }));
    assert_eq!(
        listed,
        sorted(&requested),
        "expected one record per resource the group holds, got {page}",
    );
    for item in &items {
        let arn = item["ResourceArn"].as_str().unwrap_or("<none>");
        assert_eq!(
            (item["Action"].as_str(), item["Status"].as_str()),
            (Some("GROUP"), Some("SUCCESS")),
            "case {arn:?}: expected the last action to be a successful GROUP, got {item}",
        );
    }
}
