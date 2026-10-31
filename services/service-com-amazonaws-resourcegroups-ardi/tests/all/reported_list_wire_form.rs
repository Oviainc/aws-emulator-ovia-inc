// SPDX-License-Identifier: CC0-1.0

//! The wire form of a reported list, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rule under test, asserted over a TABLE of calls rather than one row: every
//! list member a modelled output declares is REPORTED on the wire, carrying exactly
//! what the call computed for it — the empty list when it computed nothing. A length
//! declared on the member's target bounds the list a REQUEST may carry; it never
//! turns a report with nothing in it into an omission, so "this call changed nothing"
//! stays distinguishable from "this member was not reported".
//!
//! In this service that is the three members of a static-membership answer:
//! `Succeeded` (`ResourceArnList`, `length 1..10`) beside `Failed`
//! (`FailedResourceList`) and `Pending` (`PendingResourceList`), which declare no
//! length (`smithy_query(query="shape",shape="GroupResourcesOutput")`). A call that
//! changed no membership answers all three, with `Succeeded` empty — the same wire
//! form the sibling `group_resources_supported_resource_types` suite asserts for the
//! query-evaluating listings' `QueryErrors`.
//!
//! What a call reports in `Succeeded` when it DID change memberships is the sibling
//! `group_reported_additions` / `ungroup_reported_removals` rule, and the `ErrorCode`
//! and `ErrorMessage` of a `Failed` entry are the sibling
//! `group_resources_supported_resource_types` rule; both are asserted here only to
//! the extent the wire form needs them.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::group_resources_supported_resource_types::ALLOWED_RESOURCE_TYPES;
use super::resource_group_lifecycle::Emulator;
use super::ungroup_resources::{group_with_members, listed_members, sorted, succeeded};

/// Every list member a static-membership answer declares, in sorted order
/// (`smithy_query(query="shape",shape="GroupResourcesOutput,UngroupResourcesOutput")`).
const REPORTED_MEMBERS: [&str; 3] = ["Failed", "Pending", "Succeeded"];

/// The most entries `ResourceArns` and `Succeeded` carry: the maximum the modelled
/// `ResourceArnList` declares (`length 1..10`).
const MAX_RESOURCE_ARNS: usize = 10;

/// The CloudFormation-style resource type of the ARNs [`arn`] builds, as
/// `ResourceIdentifier$ResourceType` names one ("such as `AWS::EC2::Instance`").
const INSTANCE_TYPE: &str = "AWS::EC2::Instance";

/// A resource type no ARN of this suite has, so a configuration that admits only it
/// refuses every ARN below — the shape of call the replayed fixture makes.
const OTHER_TYPE: &str = "AWS::EC2::CapacityReservation";

/// Distinct resource ARNs to group and ungroup, `index` apart.
fn arn(index: usize) -> String {
    format!("arn:aws:ec2:us-east-1:000000000000:instance/i-{index:017x}")
}

/// `count` distinct resource ARNs, starting at `first`.
fn arns(first: usize, count: usize) -> Vec<String> {
    (first..first + count).map(arn).collect()
}

/// One row: the resource types the group's configuration admits, the memberships it
/// is created holding, the call made against it, and what that call is expected to
/// report in each member.
struct Case {
    label: String,
    /// `"/group-resources"` or `"/ungroup-resources"`.
    path: &'static str,
    /// The types the group's configuration admits, or `None` for a group that
    /// admits every resource.
    admits: Option<&'static str>,
    held: Vec<String>,
    request: Vec<String>,
    /// What the call changes, and so reports in `Succeeded` — empty when it changes
    /// nothing.
    changed: Vec<String>,
    /// The ARNs the call refuses, and so reports in `Failed`, in request order.
    refused: Vec<String>,
}

/// Every shape of call the rule spans, on BOTH static-membership operations: a call
/// that changes one membership, a few, and the modelled maximum of ten; a call that
/// changes none (a repeat of a grouping, a removal of what was never held) at one
/// ARN and at ten; a call that names the same ARN twice; a call that mixes an ARN it
/// changes with one it does not; and a call every ARN of which the group's
/// configuration refuses, so the answer reports nothing in one member and something
/// in another.
fn cases() -> Vec<Case> {
    let ten = arns(0, MAX_RESOURCE_ARNS);
    vec![
        Case {
            label: "a grouping of one ARN the group does not hold".to_string(),
            path: "/group-resources",
            admits: None,
            held: Vec::new(),
            request: arns(0, 1),
            changed: arns(0, 1),
            refused: Vec::new(),
        },
        Case {
            label: "a grouping of three ARNs, none of them held".to_string(),
            path: "/group-resources",
            admits: None,
            held: Vec::new(),
            request: arns(0, 3),
            changed: arns(0, 3),
            refused: Vec::new(),
        },
        Case {
            label: format!("a grouping of the modelled maximum of {MAX_RESOURCE_ARNS} ARNs"),
            path: "/group-resources",
            admits: None,
            held: Vec::new(),
            request: ten.clone(),
            changed: ten.clone(),
            refused: Vec::new(),
        },
        Case {
            label: "a grouping repeated for the one ARN the group already holds".to_string(),
            path: "/group-resources",
            admits: None,
            held: arns(0, 1),
            request: arns(0, 1),
            changed: Vec::new(),
            refused: Vec::new(),
        },
        Case {
            label: format!("a grouping repeated for all {MAX_RESOURCE_ARNS} ARNs held"),
            path: "/group-resources",
            admits: None,
            held: ten.clone(),
            request: ten.clone(),
            changed: Vec::new(),
            refused: Vec::new(),
        },
        Case {
            label: "a grouping naming the same new ARN twice in one request".to_string(),
            path: "/group-resources",
            admits: None,
            held: Vec::new(),
            request: vec![arn(0), arn(0)],
            changed: arns(0, 1),
            refused: Vec::new(),
        },
        Case {
            label: "a grouping naming the same held ARN twice in one request".to_string(),
            path: "/group-resources",
            admits: None,
            held: arns(0, 1),
            request: vec![arn(0), arn(0)],
            changed: Vec::new(),
            refused: Vec::new(),
        },
        Case {
            label: "a grouping mixing a held ARN with a new one".to_string(),
            path: "/group-resources",
            admits: None,
            held: arns(0, 1),
            request: arns(0, 2),
            changed: arns(1, 1),
            refused: Vec::new(),
        },
        Case {
            label: "a grouping of one ARN whose type the configuration refuses".to_string(),
            path: "/group-resources",
            admits: Some(OTHER_TYPE),
            held: Vec::new(),
            request: arns(0, 1),
            changed: Vec::new(),
            refused: arns(0, 1),
        },
        Case {
            label: format!(
                "a grouping of {MAX_RESOURCE_ARNS} ARNs the configuration all refuses"
            ),
            path: "/group-resources",
            admits: Some(OTHER_TYPE),
            held: Vec::new(),
            request: ten.clone(),
            changed: Vec::new(),
            refused: ten.clone(),
        },
        Case {
            label: "a grouping the configuration admits, type for type".to_string(),
            path: "/group-resources",
            admits: Some(INSTANCE_TYPE),
            held: Vec::new(),
            request: arns(0, 2),
            changed: arns(0, 2),
            refused: Vec::new(),
        },
        Case {
            label: "an ungrouping of the one membership the group holds".to_string(),
            path: "/ungroup-resources",
            admits: None,
            held: arns(0, 1),
            request: arns(0, 1),
            changed: arns(0, 1),
            refused: Vec::new(),
        },
        Case {
            label: format!("an ungrouping of all {MAX_RESOURCE_ARNS} memberships at once"),
            path: "/ungroup-resources",
            admits: None,
            held: ten.clone(),
            request: ten.clone(),
            changed: ten.clone(),
            refused: Vec::new(),
        },
        Case {
            label: "an ungrouping of an ARN the group never held".to_string(),
            path: "/ungroup-resources",
            admits: None,
            held: arns(0, 1),
            request: arns(5, 1),
            changed: Vec::new(),
            refused: Vec::new(),
        },
        Case {
            label: format!("an ungrouping of {MAX_RESOURCE_ARNS} ARNs the group never held"),
            path: "/ungroup-resources",
            admits: None,
            held: arns(0, 1),
            request: arns(20, MAX_RESOURCE_ARNS),
            changed: Vec::new(),
            refused: Vec::new(),
        },
        Case {
            label: "an ungrouping repeated for an ARN already dropped".to_string(),
            path: "/ungroup-resources",
            admits: None,
            held: Vec::new(),
            request: arns(0, 1),
            changed: Vec::new(),
            refused: Vec::new(),
        },
        Case {
            label: "an ungrouping naming the same held ARN twice in one request".to_string(),
            path: "/ungroup-resources",
            admits: None,
            held: arns(0, 1),
            request: vec![arn(0), arn(0)],
            changed: arns(0, 1),
            refused: Vec::new(),
        },
        Case {
            label: "an ungrouping mixing a held ARN with one never held".to_string(),
            path: "/ungroup-resources",
            admits: None,
            held: arns(0, 1),
            request: vec![arn(0), arn(7)],
            changed: arns(0, 1),
            refused: Vec::new(),
        },
        Case {
            label: "an ungrouping of one ARN whose type the configuration refuses".to_string(),
            path: "/ungroup-resources",
            admits: Some(OTHER_TYPE),
            held: Vec::new(),
            request: arns(0, 1),
            changed: Vec::new(),
            refused: arns(0, 1),
        },
        Case {
            label: "an ungrouping mixing a membership with an ARN the configuration refuses"
                .to_string(),
            path: "/ungroup-resources",
            admits: Some(INSTANCE_TYPE),
            held: arns(0, 1),
            request: vec![arn(0), "arn:aws:s3:::reported-list-bucket".to_string()],
            changed: arns(0, 1),
            refused: vec!["arn:aws:s3:::reported-list-bucket".to_string()],
        },
    ]
}

/// The configuration a row's group is created with: one of the configuration types
/// a static-membership operation serves, plus — when the row restricts the group —
/// the `allowed-resource-types` parameter naming the types it admits (AWS
/// service-configuration vocabulary, as the sibling
/// `group_resources_supported_resource_types` suite cites).
fn configuration(admits: Option<&str>) -> Value {
    match admits {
        None => json!([{ "Type": "AWS::EC2::HostManagement" }]),
        Some(resource_type) => json!([
            { "Type": "AWS::EC2::CapacityReservationPool" },
            {
                "Type": "AWS::ResourceGroups::Generic",
                "Parameters": [
                    { "Name": ALLOWED_RESOURCE_TYPES, "Values": [resource_type] }
                ]
            }
        ]),
    }
}

/// Create the row's group and record the memberships it starts with.
async fn group_for(emulator: &Emulator, name: &str, case: &Case) {
    if case.admits.is_none() {
        group_with_members(emulator, name, &case.held).await;
        return;
    }
    emulator
        .ok(
            "/groups",
            &json!({ "Name": name, "Configuration": configuration(case.admits) }),
        )
        .await;
    // A restricted row holds at most the modelled maximum, so one call records it.
    if !case.held.is_empty() {
        emulator
            .ok(
                "/group-resources",
                &json!({ "Group": name, "ResourceArns": case.held }),
            )
            .await;
    }
}

/// The memberships the group holds once the call has been served: a grouping adds
/// what it reported adding, an ungrouping drops what it reported dropping. Derived
/// from the row, not from the answer.
fn expected_members(case: &Case) -> Vec<String> {
    let mut members: BTreeSet<String> = case.held.iter().cloned().collect();
    for changed in &case.changed {
        if case.path == "/group-resources" {
            members.insert(changed.clone());
        } else {
            members.remove(changed);
        }
    }
    members.into_iter().collect()
}

/// The `ResourceArn` of every `Failed` entry of an answer, in the order the answer
/// carries them. The member is required to be on the wire.
fn failed_arns(case: &str, answered: &Value) -> Vec<String> {
    answered
        .get("Failed")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a Failed list, got {answered}"))
        .iter()
        .map(|report| {
            report["ResourceArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a ResourceArn on every Failed entry, got {report}")
                })
                .to_string()
        })
        .collect()
}

/// The rule: every declared list member is on the wire, carrying what the call
/// computed for it — so a call that changed no membership answers `Succeeded` as the
/// empty list rather than leaving it out, and a call that refused every ARN it was
/// given answers an empty `Succeeded` beside a populated `Failed`.
#[tokio::test]
async fn every_reported_list_member_is_answered_even_when_it_has_nothing_in_it() {
    for case in cases() {
        let label = case.label.as_str();
        let name = "reported-list-probe";
        let emulator = Emulator::new();
        group_for(&emulator, name, &case).await;

        let answered = emulator
            .ok(
                case.path,
                &json!({ "Group": name, "ResourceArns": case.request }),
            )
            .await;
        let body = answered
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: expected a JSON object, got {answered}"));
        // Every member the output declares is present, and is a list, whatever the
        // call did.
        for member in REPORTED_MEMBERS {
            assert!(
                body.get(member).is_some_and(Value::is_array),
                "case {label:?}: expected {member} to be answered as a list, got {answered}",
            );
        }
        let expected_changed = sorted(&case.changed);
        let reported = succeeded(label, &answered);
        assert_eq!(
            reported, expected_changed,
            "case {label:?}: expected Succeeded to be {expected_changed:?}, got {answered}",
        );
        // The declared maximum bounds what one answer can report; the declared
        // MINIMUM bounds the `ResourceArns` a request may carry, not this report.
        assert!(
            reported.len() <= MAX_RESOURCE_ARNS,
            "case {label:?}: expected at most {MAX_RESOURCE_ARNS} entries in Succeeded, \
             got {} in {answered}",
            reported.len(),
        );
        assert_eq!(
            failed_arns(label, &answered),
            case.refused,
            "case {label:?}: expected Failed to name {:?}, got {answered}",
            case.refused,
        );
        assert_eq!(
            body.get("Pending"),
            Some(&json!([])),
            "case {label:?}: a call this emulator applies in full leaves nothing pending, \
             got {answered}",
        );
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

/// An empty `Succeeded` is the answer to EVERY repeat, not only the first, and the
/// list fills again as soon as a call changes a membership: the member is always
/// there and its CONTENTS follow what the call did.
#[tokio::test]
async fn an_empty_report_is_answered_on_every_repeat() {
    let emulator = Emulator::new();
    let name = "reported-list-rounds";
    group_with_members(&emulator, name, &[]).await;
    let member = arn(0);

    // Each round: the call, and the memberships it changes.
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
        let expected: Vec<String> = if changes {
            vec![member.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            succeeded(label, &answered),
            expected,
            "case {label:?}: expected Succeeded to be {expected:?}, got {answered}",
        );
    }
}

/// The report is a property of the ANSWER's shape and not of the call having done
/// something: a call that changed nothing carries exactly the members the model
/// declares, so a client reading the answer as a map finds every one of them.
#[tokio::test]
async fn an_answer_that_changed_nothing_still_carries_every_member() {
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
        REPORTED_MEMBERS.to_vec(),
        "expected every member the output declares, got {answered}",
    );
}
