// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the group-membership REMOVAL path: `DisassociateResource`, and
//! the reads that prove what it wrote (`ListGroupResources`, `ListAssociatedGroups`,
//! `GetGroup`, `GetCanary`).
//!
//! The rules asserted here, each stated once and then exercised over a SET of inputs
//! rather than over one recorded request:
//!
//! * `DisassociateResource` deletes ONE membership — the pair (group, canary ARN) the
//!   request names — from the group's membership set, and nothing else: the group's other
//!   members stay, the other groups holding that canary keep it, and both resources
//!   themselves survive the call.
//! * The group is addressed by any of the three forms the model documents for a
//!   `GroupIdentifier` — "You can specify the group name, the ARN, or the group ID" — and
//!   all three reach the ONE stored group.
//! * Membership is a set, so removing a canary that is not in the group succeeds and
//!   changes nothing, including `Group$LastModifiedTime`; and a removal can be undone by
//!   the paired write, `AssociateResource`.
//! * Both sides of the membership must exist: a `GroupIdentifier` no group answers to and
//!   a `ResourceArn` no canary answers to are each refused with the modelled 404, worded
//!   from the request's own identifier, and a refusal changes no membership.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, through the harness the lifecycle suite defines.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::json;

use crate::canary_lifecycle::{Answer, Synthetics};
use crate::group_lifecycle::{audit_capture, group_not_found_message};
use crate::group_membership::{
    associate, canary, canary_arn, canary_not_found_message, create_group, error_of,
    list_resources, ok, resources_of,
};

/// `CanaryName` is modelled `length 1..255, pattern ^[0-9a-z_\-]+$`
/// (`smithy_query(query="shape",shape="CanaryName")`), which bounds the canary segment of a
/// `CanaryArn` — so an ARN naming a canary of this length is still a legal `ResourceArn`.
const MAX_CANARY_NAME_LEN: usize = 255;

/// Long enough that a wrongly bumped `LastModifiedTime` would be visible at sub-second
/// resolution, short enough to keep the suite quick.
const OBSERVABLE_GAP: Duration = Duration::from_millis(30);

/// `PATCH /group/{GroupIdentifier}/disassociate` — the `DisassociateResource` route, from
/// the operation's own `@http` binding
/// (`smithy_query(query="operation",operation="DisassociateResource")`).
async fn disassociate(aws: &Synthetics, identifier: &str, resource_arn: &str) -> Answer {
    aws.call(
        "PATCH",
        &format!("/group/{identifier}/disassociate"),
        Some(json!({ "ResourceArn": resource_arn })),
    )
    .await
}

/// `POST /resource/{ResourceArn}/groups` — the `ListAssociatedGroups` route.
async fn list_associated(aws: &Synthetics, resource_arn: &str) -> Answer {
    aws.call(
        "POST",
        &format!("/resource/{resource_arn}/groups"),
        Some(json!({})),
    )
    .await
}

async fn get_group(aws: &Synthetics, identifier: &str) -> Answer {
    aws.call("GET", &format!("/group/{identifier}"), None).await
}

/// The membership set `ListGroupResources` reports for `identifier`.
async fn members(case: &str, aws: &Synthetics, identifier: &str) -> BTreeSet<String> {
    let listed = ok(case, list_resources(aws, identifier, json!({})).await);
    resources_of(case, &listed).into_iter().collect()
}

/// The group NAMES `ListAssociatedGroups` reports for `resource_arn`.
async fn groups_of_canary(case: &str, aws: &Synthetics, resource_arn: &str) -> BTreeSet<String> {
    let listed = ok(case, list_associated(aws, resource_arn).await);
    listed
        .at("Groups")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry["Name"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case}: expected a group Name in {entry:?}"))
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn set_of<'a>(arns: impl IntoIterator<Item = &'a String>) -> BTreeSet<String> {
    arns.into_iter().cloned().collect()
}

/// `Group$LastModifiedTime` as the store reports it.
async fn last_modified(case: &str, aws: &Synthetics, identifier: &str) -> f64 {
    let read = ok(case, get_group(aws, identifier).await);
    read.at("Group.LastModifiedTime")
        .as_f64()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a numeric Group.LastModifiedTime, got {:?}",
                read.at("Group.LastModifiedTime")
            )
        })
}

/// One removal takes exactly the named membership out of the group, whichever of the three
/// `GroupIdentifier` forms addresses it — and leaves the group's other members, the canary
/// itself and the group's identity alone.
#[tokio::test]
async fn disassociate_resource_removes_the_named_membership_by_every_identifier_form() {
    let region = "eu-west-1";
    let aws = Synthetics::new(region);

    let created = ok(
        "create-group",
        create_group(&aws, json!({ "Name": "web-probes" })).await,
    );
    let name = created.string_at("Group.Name");
    let id = created.string_at("Group.Id");
    let arn = created.string_at("Group.Arn");

    // Three canaries, so each row below removes a DIFFERENT membership and the set that is
    // left proves the other two were untouched. The names differ along `CanaryName`'s
    // charset (hyphen, underscore) as well.
    let alpha = canary(&aws, region, "alpha-canary").await;
    let beta = canary(&aws, region, "beta-canary").await;
    let gamma = canary(&aws, region, "gamma_canary").await;
    let all = [alpha.clone(), beta.clone(), gamma.clone()];
    for member in &all {
        ok(
            &format!("associate/{member}"),
            associate(&aws, &id, member).await,
        );
    }

    // The rows differ along the form of the identifier, which is what the rule generalises
    // over; each one removes a different canary from the one stored group.
    let rows = [
        ("by-name", name.clone(), alpha.clone()),
        ("by-id", id.clone(), beta.clone()),
        ("by-arn", arn.clone(), gamma.clone()),
    ];

    let mut expected: BTreeSet<String> = set_of(all.iter());
    for (case, identifier, resource) in &rows {
        ok(
            &format!("disassociate/{case}"),
            disassociate(&aws, identifier, resource).await,
        );
        expected.remove(resource);

        // Read the set back through every identifier form, so the removal landed on the
        // one stored group rather than on a copy addressed by the form used.
        for (reader, form) in [("by-name", &name), ("by-id", &id), ("by-arn", &arn)] {
            let reported = members(
                &format!("list/{case}/{reader}"),
                &aws,
                form,
            )
            .await;
            assert_eq!(
                reported, expected,
                "case disassociate/{case} read {reader}: expected the membership set \
                 {expected:?} after removing {resource}, got {reported:?}"
            );
        }

        // The other side of the association reports the same removal.
        let still_in = groups_of_canary(&format!("associated/{case}"), &aws, resource).await;
        assert!(
            still_in.is_empty(),
            "case disassociate/{case}: expected {resource} to be a member of no group, \
             got {still_in:?}"
        );

        // A membership was removed, not a canary: the canary is still there to re-associate.
        let read_back = aws
            .get(resource.split(':').next_back().unwrap_or_default())
            .await;
        assert_eq!(
            read_back.status, 200,
            "case disassociate/{case}: expected the canary to survive the removal, got {} ({:?})",
            read_back.status, read_back.body
        );
    }

    // The group's own identity never moved while its membership emptied.
    let after = ok("get-group/after", get_group(&aws, &id).await);
    for field in ["Group.Id", "Group.Name", "Group.Arn", "Group.CreatedTime"] {
        assert_eq!(
            after.at(field),
            created.at(field),
            "case get-group/after: expected {field} unchanged, got {:?} vs {:?}",
            after.at(field),
            created.at(field)
        );
    }
}

/// Membership is a set: removing a canary the group does not hold succeeds and changes
/// nothing (the set, and `LastModifiedTime` with it), a removal touches only the one group
/// named, and `AssociateResource` puts back what a removal took out.
#[tokio::test]
async fn disassociate_resource_removes_only_one_membership_and_repeats_harmlessly() {
    let region = "us-east-1";
    let aws = Synthetics::new(region);

    let left = ok(
        "create-group/left",
        create_group(&aws, json!({ "Name": "left-group" })).await,
    )
    .string_at("Group.Id");
    let right = ok(
        "create-group/right",
        create_group(&aws, json!({ "Name": "right-group" })).await,
    )
    .string_at("Group.Id");

    let first = canary(&aws, region, "first-canary").await;
    let second = canary(&aws, region, "second-canary").await;
    // A canary that is a member of nothing, so a removal can be asked for a pair that was
    // never associated.
    let stranger = canary(&aws, region, "stranger-canary").await;
    for group in [&left, &right] {
        for member in [&first, &second] {
            ok(
                &format!("associate/{group}/{member}"),
                associate(&aws, group, member).await,
            );
        }
    }

    // Rows that must each leave the membership sets exactly as the row before them did:
    // a repeat of a removal already made, and a removal of a pair never associated.
    let one_removed: BTreeSet<String> = set_of([&second]);
    let both: BTreeSet<String> = set_of([&first, &second]);
    ok(
        "disassociate/first-from-left",
        disassociate(&aws, &left, &first).await,
    );

    let no_ops = [
        ("repeat", first.clone()),
        ("never-associated", stranger.clone()),
    ];
    for (case, resource) in &no_ops {
        let before = last_modified(&format!("before/{case}"), &aws, &left).await;
        tokio::time::sleep(OBSERVABLE_GAP).await;
        ok(
            &format!("disassociate/{case}"),
            disassociate(&aws, &left, resource).await,
        );

        let reported = members(&format!("list/{case}"), &aws, &left).await;
        assert_eq!(
            reported, one_removed,
            "case {case}: expected the membership set {one_removed:?} to be unchanged by a \
             removal of {resource}, got {reported:?}"
        );
        let after = last_modified(&format!("after/{case}"), &aws, &left).await;
        assert_eq!(
            after, before,
            "case {case}: expected LastModifiedTime unchanged by a removal that removed \
             nothing, got {after} vs {before}"
        );
    }

    // The other group kept the membership the removals above did not name.
    let untouched = members("list/right", &aws, &right).await;
    assert_eq!(
        untouched, both,
        "case list/right: expected the other group to keep {both:?}, got {untouched:?}"
    );
    let stranger_groups = groups_of_canary("associated/stranger", &aws, &stranger).await;
    assert!(
        stranger_groups.is_empty(),
        "case associated/stranger: expected a canary no call associated to be in no group, \
         got {stranger_groups:?}"
    );
    let first_groups = groups_of_canary("associated/first", &aws, &first).await;
    assert_eq!(
        first_groups,
        BTreeSet::from(["right-group".to_string()]),
        "case associated/first: expected only the group the removals left, got {first_groups:?}"
    );

    // What a removal took out, the paired write puts back.
    ok(
        "re-associate",
        associate(&aws, &left, &first).await,
    );
    let restored = members("list/after-re-associate", &aws, &left).await;
    assert_eq!(
        restored, both,
        "case list/after-re-associate: expected {both:?} after re-associating, got {restored:?}"
    );
}

/// Both sides of the membership have to exist: a group identifier no group answers to and a
/// canary ARN no canary answers to are each refused with the modelled 404, worded from the
/// request's own identifier — and a refusal removes nothing.
#[tokio::test]
async fn disassociate_resource_reports_a_missing_group_or_canary_as_not_found() {
    let region = "us-west-2";
    let account = engine::EMULATOR_ACCOUNT_ID;
    let aws = Synthetics::new(region);

    let live_arn = canary(&aws, region, "live-canary").await;
    let live_id = ok(
        "create-group",
        create_group(&aws, json!({ "Name": "live-group" })).await,
    )
    .string_at("Group.Id");
    ok("associate", associate(&aws, &live_id, &live_arn).await);

    // Group identifiers no group answers to, spanning the three forms the member accepts,
    // the modelled length bound (`GroupIdentifier` is `len 1..128`) and resemblance to the
    // group that DOES exist.
    let missing_groups = [
        ("single-character", "x".to_string()),
        ("name-like", "no-such-group".to_string()),
        ("id-like", "0123456789ab".to_string()),
        (
            "arn-like",
            format!("arn:aws:synthetics:{region}:{account}:group:deadbeefcafe"),
        ),
        ("max-length", "g".repeat(128)),
    ];
    for (case, identifier) in &missing_groups {
        let answer = disassociate(&aws, identifier, &live_arn).await;
        let message = error_of(case, &answer, 404, "ResourceNotFoundException");
        assert_eq!(
            message.as_deref(),
            Some(group_not_found_message(identifier).as_str()),
            "case {case}: expected the missing-group sentence naming the identifier as \
             supplied, got {message:?}"
        );
    }

    // Legal `CanaryArn`s no store holds, differing along the canary name's charset and
    // length, the Region ("You must run this operation in the Region where the canary
    // exists") and the account.
    let missing_canaries = [
        ("another-name", canary_arn(region, "no-such-canary")),
        ("single-character-name", canary_arn(region, "x")),
        ("underscored-name", canary_arn(region, "no_such_1")),
        (
            "max-length-name",
            canary_arn(region, &"c".repeat(MAX_CANARY_NAME_LEN)),
        ),
        ("another-region", canary_arn("eu-west-3", "live-canary")),
        (
            "another-account",
            format!("arn:aws:synthetics:{region}:999999999999:canary:live-canary"),
        ),
    ];
    for (case, resource_arn) in &missing_canaries {
        let answer = disassociate(&aws, &live_id, resource_arn).await;
        let message = error_of(case, &answer, 404, "ResourceNotFoundException");
        assert_eq!(
            message.as_deref(),
            Some(canary_not_found_message(resource_arn).as_str()),
            "case {case}: expected the missing-canary sentence naming the ARN as supplied, \
             got {message:?}"
        );
    }

    // Every row above was refused, so the one membership that exists still does.
    let reported = members("list/after-refusals", &aws, &live_id).await;
    assert_eq!(
        reported,
        BTreeSet::from([live_arn.clone()]),
        "case list/after-refusals: expected the refusals to remove nothing, got {reported:?}"
    );

    // And the removal that CAN be made still is.
    ok(
        "disassociate/live",
        disassociate(&aws, &live_id, &live_arn).await,
    );
    let emptied = members("list/after-live", &aws, &live_id).await;
    assert!(
        emptied.is_empty(),
        "case list/after-live: expected an empty membership set, got {emptied:?}"
    );
}

/// The target `state::AUDIT_TARGET` names, written here as a literal so the assertions pin
/// the wire-visible target rather than whatever the constant happens to hold.
const AUDIT_TARGET: &str = "emulator::audit";

/// The Region this suite's audit test alone signs for, so its records can be told apart
/// from those of the sibling suites running in parallel in this binary.
const AUDIT_REGION: &str = "ap-south-1";

/// Every captured `emulator::audit` record a `DisassociateResource` call signed for
/// `region` wrote. The capture is read without draining it, because sibling audit suites
/// read the same global capture as a delta.
fn disassociate_records(captured: &[String], region: &str) -> Vec<String> {
    captured
        .iter()
        .filter(|record| {
            record.starts_with(&format!("{AUDIT_TARGET} "))
                && record.contains("operation=DisassociateResource")
                && record.contains(&format!("region=\"{region}\""))
        })
        .cloned()
        .collect()
}

/// RULE: every `DisassociateResource` call leaves exactly ONE structured record on the
/// audit target, naming the operation, the identity the request presented, the Region it
/// resolved to and the outcome — and nothing the caller supplied. A table over every
/// outcome the operation can report.
#[tokio::test]
async fn every_disassociate_resource_call_emits_one_structured_audit_record() {
    // The same global capture the group-lifecycle audit suites install: a test binary can
    // hold only one global `tracing` subscriber, so it is shared rather than re-installed,
    // and read as a delta rather than drained.
    let captured = audit_capture();

    let aws = Synthetics::new(AUDIT_REGION);
    let group_id = ok(
        "create-group",
        create_group(&aws, json!({ "Name": "audited-membership-group" })).await,
    )
    .string_at("Group.Id");
    let member = canary(&aws, AUDIT_REGION, "audited-canary").await;
    ok("associate", associate(&aws, &group_id, &member).await);

    // (case, the identifier addressed, the resource named, the outcome it must record,
    // the status). The rows span every outcome: a removal, a removal that removed nothing,
    // and each side of the membership missing.
    let cases = [
        ("removed", group_id.clone(), member.clone(), "removed", 200),
        ("repeat", group_id.clone(), member.clone(), "no_change", 200),
        (
            "group-missing",
            "no-such-audited-group".to_string(),
            member.clone(),
            "rejected_group_not_found",
            404,
        ),
        (
            "canary-missing",
            group_id.clone(),
            canary_arn(AUDIT_REGION, "no-such-audited-canary"),
            "rejected_canary_not_found",
            404,
        ),
    ];

    let before = disassociate_records(&captured.all(), AUDIT_REGION).len();
    for (index, (case, identifier, resource, outcome, status)) in cases.iter().enumerate() {
        let seen = before + index;
        let answer = disassociate(&aws, identifier, resource).await;
        assert_eq!(
            answer.status, *status,
            "case {case}: expected {status}, got {} ({:?})",
            answer.status, answer.body
        );

        let records = disassociate_records(&captured.all(), AUDIT_REGION);
        assert_eq!(
            records.len(),
            seen + 1,
            "case {case}: expected exactly one new audit record, got {:?}",
            &records[seen.min(records.len())..]
        );
        let record = records[seen].clone();

        for field in [
            "operation=DisassociateResource".to_string(),
            format!("outcome={outcome}"),
            format!("region=\"{AUDIT_REGION}\""),
            "caller=".to_string(),
        ] {
            assert!(
                record.contains(&field),
                "case {case}: expected the audit record to carry {field:?}, got {record:?}"
            );
        }

        // Metadata only: nothing the caller supplied reaches the trail.
        assert!(
            !record.contains(identifier.as_str()) && !record.contains(resource.as_str()),
            "case {case}: expected no caller-supplied identifier in the audit record, \
             got {record:?}"
        );
    }
}

