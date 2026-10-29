// SPDX-License-Identifier: CC0-1.0

//! The wire form of a reported list, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rule under test, asserted over a TABLE of calls rather than one row: an
//! output list member whose modelled target declares a length MINIMUM carries no
//! empty list, so a call with nothing to report in it leaves the member out of the
//! answer; a member whose target declares no minimum may carry an empty list.
//!
//! In this service that is `Succeeded`, on both static-membership operations: it
//! targets `ResourceArnList`, `length 1..10`
//! (`smithy_query(query="shape",shape="ResourceArnList")`), while its siblings
//! `Failed` (`FailedResourceList`) and `Pending` (`PendingResourceList`) declare no
//! length at all — so a call that changed no membership answers no `Succeeded`
//! member and still answers `Failed` and `Pending` as empty lists.
//!
//! What a call reports in `Succeeded` when it DID change memberships is the sibling
//! `group_reported_additions` / `ungroup_reported_removals` rule and is asserted
//! here only to the extent the wire form needs it.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::resource_group_lifecycle::Emulator;
use super::ungroup_resources::{group_with_members, listed_members, sorted, succeeded};

/// The most entries `ResourceArns` and `Succeeded` carry: the maximum the modelled
/// `ResourceArnList` declares (`length 1..10`).
const MAX_RESOURCE_ARNS: usize = 10;

/// The least entries `Succeeded` can carry when it is present at all: the minimum
/// the modelled `ResourceArnList` declares (`length 1..10`).
const MIN_REPORTED_ARNS: usize = 1;

/// Distinct resource ARNs to group and ungroup, `index` apart.
fn arn(index: usize) -> String {
    format!("arn:aws:ec2:us-east-1:000000000000:instance/i-{index:017x}")
}

/// `count` distinct resource ARNs, starting at `first`.
fn arns(first: usize, count: usize) -> Vec<String> {
    (first..first + count).map(arn).collect()
}

/// One row: the memberships the group is created holding, the call made against it,
/// and the memberships that call is expected to CHANGE (add for a grouping, drop for
/// an ungrouping).
struct Case {
    label: String,
    /// `"/group-resources"` or `"/ungroup-resources"`.
    path: &'static str,
    held: Vec<String>,
    request: Vec<String>,
    /// What the call changes, and so reports — empty when it changes nothing.
    changed: Vec<String>,
}

/// Every shape of call the rule spans, on BOTH static-membership operations: a call
/// that changes one membership, a few, and the modelled maximum of ten; a call that
/// changes none (a repeat of a grouping, a removal of what was never held) at one
/// ARN and at ten; a call that names the same ARN twice; and a call that mixes an
/// ARN it changes with one it does not.
fn cases() -> Vec<Case> {
    let ten = arns(0, MAX_RESOURCE_ARNS);
    vec![
        Case {
            label: "a grouping of one ARN the group does not hold".to_string(),
            path: "/group-resources",
            held: Vec::new(),
            request: arns(0, 1),
            changed: arns(0, 1),
        },
        Case {
            label: "a grouping of three ARNs, none of them held".to_string(),
            path: "/group-resources",
            held: Vec::new(),
            request: arns(0, 3),
            changed: arns(0, 3),
        },
        Case {
            label: format!("a grouping of the modelled maximum of {MAX_RESOURCE_ARNS} ARNs"),
            path: "/group-resources",
            held: Vec::new(),
            request: ten.clone(),
            changed: ten.clone(),
        },
        Case {
            label: "a grouping repeated for the one ARN the group already holds".to_string(),
            path: "/group-resources",
            held: arns(0, 1),
            request: arns(0, 1),
            changed: Vec::new(),
        },
        Case {
            label: format!("a grouping repeated for all {MAX_RESOURCE_ARNS} ARNs held"),
            path: "/group-resources",
            held: ten.clone(),
            request: ten.clone(),
            changed: Vec::new(),
        },
        Case {
            label: "a grouping naming the same new ARN twice in one request".to_string(),
            path: "/group-resources",
            held: Vec::new(),
            request: vec![arn(0), arn(0)],
            changed: arns(0, 1),
        },
        Case {
            label: "a grouping naming the same held ARN twice in one request".to_string(),
            path: "/group-resources",
            held: arns(0, 1),
            request: vec![arn(0), arn(0)],
            changed: Vec::new(),
        },
        Case {
            label: "a grouping mixing a held ARN with a new one".to_string(),
            path: "/group-resources",
            held: arns(0, 1),
            request: arns(0, 2),
            changed: arns(1, 1),
        },
        Case {
            label: "an ungrouping of the one membership the group holds".to_string(),
            path: "/ungroup-resources",
            held: arns(0, 1),
            request: arns(0, 1),
            changed: arns(0, 1),
        },
        Case {
            label: format!("an ungrouping of all {MAX_RESOURCE_ARNS} memberships at once"),
            path: "/ungroup-resources",
            held: ten.clone(),
            request: ten.clone(),
            changed: ten.clone(),
        },
        Case {
            label: "an ungrouping of an ARN the group never held".to_string(),
            path: "/ungroup-resources",
            held: arns(0, 1),
            request: arns(5, 1),
            changed: Vec::new(),
        },
        Case {
            label: format!("an ungrouping of {MAX_RESOURCE_ARNS} ARNs the group never held"),
            path: "/ungroup-resources",
            held: arns(0, 1),
            request: arns(20, MAX_RESOURCE_ARNS),
            changed: Vec::new(),
        },
        Case {
            label: "an ungrouping repeated for an ARN already dropped".to_string(),
            path: "/ungroup-resources",
            held: Vec::new(),
            request: arns(0, 1),
            changed: Vec::new(),
        },
        Case {
            label: "an ungrouping naming the same held ARN twice in one request".to_string(),
            path: "/ungroup-resources",
            held: arns(0, 1),
            request: vec![arn(0), arn(0)],
            changed: arns(0, 1),
        },
        Case {
            label: "an ungrouping mixing a held ARN with one never held".to_string(),
            path: "/ungroup-resources",
            held: arns(0, 1),
            request: vec![arn(0), arn(7)],
            changed: arns(0, 1),
        },
    ]
}

/// The memberships the group holds once the call has been served: a grouping adds
/// every ARN it named, an ungrouping drops every ARN it named. Derived from the
/// request and the group's prior memberships, not from what the answer reported.
fn expected_members(case: &Case) -> Vec<String> {
    let mut members: BTreeSet<String> = case.held.iter().cloned().collect();
    for requested in &case.request {
        if case.path == "/group-resources" {
            members.insert(requested.clone());
        } else {
            members.remove(requested);
        }
    }
    members.into_iter().collect()
}

/// The rule: `Succeeded` is present exactly when the call changed a membership, and
/// is left OUT of the answer when it changed none, while `Failed` and `Pending` —
/// whose targets declare no length — are always present and empty.
#[tokio::test]
async fn a_length_bounded_reported_list_is_omitted_rather_than_sent_empty() {
    for case in cases() {
        let label = case.label.as_str();
        let name = "reported-list-probe";
        let emulator = Emulator::new();
        group_with_members(&emulator, name, &case.held).await;

        let answered = emulator
            .ok(
                case.path,
                &json!({ "Group": name, "ResourceArns": case.request }),
            )
            .await;
        let body = answered
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: expected a JSON object, got {answered}"));
        let expected_changed = sorted(&case.changed);

        if expected_changed.is_empty() {
            assert!(
                !body.contains_key("Succeeded"),
                "case {label:?}: a call that changed no membership must carry no Succeeded \
                 member, got {answered}",
            );
        } else {
            let reported = succeeded(label, &answered);
            assert_eq!(
                reported, expected_changed,
                "case {label:?}: expected Succeeded to be {expected_changed:?}, got {answered}",
            );
            assert!(
                (MIN_REPORTED_ARNS..=MAX_RESOURCE_ARNS).contains(&reported.len()),
                "case {label:?}: expected a Succeeded of {MIN_REPORTED_ARNS}..={MAX_RESOURCE_ARNS} \
                 entries, got {} in {answered}",
                reported.len(),
            );
        }
        // The two siblings declare no length minimum, so "nothing to report" is an
        // empty list for them whatever the call did.
        for sibling in ["Failed", "Pending"] {
            assert_eq!(
                body.get(sibling),
                Some(&json!([])),
                "case {label:?}: expected {sibling} to be an empty list, got {answered}",
            );
        }
        // Round trip: the wire form of the report says nothing about the state the
        // call left, which a read of the group answers.
        let stored = listed_members(&emulator, label, name).await;
        let expected_stored = expected_members(&case);
        assert_eq!(
            stored, expected_stored,
            "case {label:?}: expected the group to hold {expected_stored:?}, got {stored:?}",
        );
    }
}

/// An omitted `Succeeded` is the answer to EVERY repeat, not only the first: the
/// member comes back as soon as a call changes a membership again, so the wire form
/// follows what the call did and not how many calls preceded it.
#[tokio::test]
async fn the_member_returns_as_soon_as_a_call_changes_a_membership_again() {
    let emulator = Emulator::new();
    let name = "reported-list-rounds";
    group_with_members(&emulator, name, &[]).await;
    let member = arn(0);

    // Each round: the call, and whether it changes a membership.
    let rounds: [(&str, &str, bool); 6] = [
        ("the first grouping", "/group-resources", true),
        ("the second grouping", "/group-resources", false),
        ("the third grouping", "/group-resources", false),
        ("the first ungrouping", "/ungroup-resources", true),
        ("the second ungrouping", "/ungroup-resources", false),
        ("a grouping after the removal", "/group-resources", true),
    ];
    for (label, path, changes) in rounds {
        let answered = emulator
            .ok(path, &json!({ "Group": name, "ResourceArns": [member] }))
            .await;
        let present = answered
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: expected a JSON object, got {answered}"))
            .contains_key("Succeeded");
        assert_eq!(
            present, changes,
            "case {label:?}: expected Succeeded to be present={changes}, got {answered}",
        );
        if changes {
            assert_eq!(
                succeeded(label, &answered),
                vec![member.clone()],
                "case {label:?}: expected Succeeded to name the one changed membership, \
                 got {answered}",
            );
        }
    }
}

/// The omission is a property of the ANSWER's shape and not of the body being
/// otherwise empty: `Failed` and `Pending` are still there, so a client reading the
/// answer as a map sees exactly the members the model admits.
#[tokio::test]
async fn an_answer_that_changed_nothing_still_carries_its_unbounded_members() {
    let emulator = Emulator::new();
    let name = "reported-list-members";
    group_with_members(&emulator, name, &arns(0, 1)).await;
    let answered: Value = emulator
        .ok(
            "/group-resources",
            &json!({ "Group": name, "ResourceArns": arns(0, 1) }),
        )
        .await;
    let mut members: Vec<&str> = answered
        .as_object()
        .unwrap_or_else(|| panic!("expected a JSON object, got {answered}"))
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        vec!["Failed", "Pending"],
        "expected only the members whose targets admit an empty list, got {answered}",
    );
}
