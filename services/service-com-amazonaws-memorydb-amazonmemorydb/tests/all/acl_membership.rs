// SPDX-License-Identifier: CC0-1.0

//! How `UpdateACL` CLASSIFIES a membership change it cannot apply, end to end over the
//! wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rule under test, over a SET of changes rather than one recorded request: a
//! membership change is applied only when it holds together AND agrees with the
//! membership the ACL has right now, and a change that does not is refused with the
//! modelled fault for THAT mistake, leaving the ACL exactly as it was:
//!
//! * a name the change ADDS that the ACL already lists contradicts the membership it
//!   would be applied to — `InvalidParameterCombinationException`, which is what AWS
//!   answers for it (recorded unit `memorydb.acl-membership-updates-rejected`), and the
//!   same fault a change naming one user on both sides, or naming no change at all,
//!   gets;
//! * a name the REQUEST ITSELF repeats within one list is a bad list value —
//!   `InvalidParameterValueException`, the same fault a removal naming a user that is
//!   not in this ACL gets;
//! * an ACL name that is in no state at all — `ACLNotFoundFault`.
//!
//! Resource names FOLD (`users_acls_name_rules`), so each row is also exercised in a
//! spelling other than the one the ACL was created with: the fault a change gets is
//! decided by the resource it names, not by how the caller spelled it.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The suite-wide harness: ONE SigV4 request builder, ONE router constructor and ONE
// error-code reader for every suite in this crate; and ONE user-creating call.
use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::user_lifecycle::create_user;

/// Longer than the service's `SETTLE_AFTER`, so the transition the clock owes an ACL is
/// certainly due by the time the test looks again. The emulator stores the due instant
/// and never sleeps; this blocks the test thread rather than awaiting, because the
/// workspace's `tokio` is declared without the `time` feature.
const PAST_SETTLE: Duration = Duration::from_millis(700);

/// The ACL every case works on, and the members it holds to begin with.
const ACL: &str = "membership-acl";
const INITIAL_MEMBERS: &[&str] = &["member-one", "member-two"];

/// A user that exists but belongs to no ACL, so a change naming it is about the
/// MEMBERSHIP rather than about whether the user exists.
const OUTSIDER: &str = "member-three";

/// One row of the accepted-change table: the case name, the names joining the ACL, the
/// names leaving it, and the membership the ACL holds once the change has been applied.
type ChangeCase<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>);

/// An `UpdateACL` body carrying only the lists the change names (`UserNameListInput` is
/// `len 1..`, so an empty list is not a list the model accepts).
fn update_body(acl_name: &str, to_add: &[&str], to_remove: &[&str]) -> Value {
    let mut body = json!({ "ACLName": acl_name });
    if !to_add.is_empty() {
        body["UserNamesToAdd"] = json!(to_add);
    }
    if !to_remove.is_empty() {
        body["UserNamesToRemove"] = json!(to_remove);
    }
    body
}

/// The one ACL `DescribeACLs` reports by name.
async fn described_acl(router: &MultiServiceRouter, acl_name: &str, case: &str) -> Value {
    let body = ok(
        router,
        "DescribeACLs",
        &json!({ "ACLName": acl_name }),
        case,
    )
    .await;
    body["ACLs"][0].clone()
}

/// The `UserNames` of an `ACL` body, in sorted order, so a comparison is about the
/// membership rather than about the order the changes arrived in.
fn members(acl: &Value, case: &str) -> Vec<String> {
    let mut names: Vec<String> = acl
        .get("UserNames")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected UserNames in {acl}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect();
    names.sort();
    names
}

/// The users every case draws on, plus the ACL holding [`INITIAL_MEMBERS`], waited out
/// to `active` so it can take a change.
async fn settled_acl(router: &MultiServiceRouter) {
    for name in INITIAL_MEMBERS.iter().chain([&OUTSIDER]) {
        create_user(router, name).await;
    }
    ok(
        router,
        "CreateACL",
        &json!({ "ACLName": ACL, "UserNames": INITIAL_MEMBERS }),
        "setup",
    )
    .await;
    std::thread::sleep(PAST_SETTLE);
    let acl = described_acl(router, ACL, "setup").await;
    assert_eq!(
        acl["Status"],
        json!("active"),
        "case \"setup\": the ACL must be active before a change is applied, got {acl}"
    );
}

/// A change that does not hold against the ACL's current membership is refused with the
/// modelled fault for the mistake it made, and the ACL is left exactly as it was.
#[tokio::test]
async fn a_refused_membership_change_reports_which_mistake_it_made() {
    let router = router();
    settled_acl(&router).await;

    // (case, the change, the fault it must get) — the rows differ in which list the
    // change uses, how many names it carries, which side of the current membership each
    // name is on, and how the name is spelled.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a member added that the ACL already lists",
            update_body(ACL, &[INITIAL_MEMBERS[0]], &[]),
            "InvalidParameterCombinationException",
        ),
        (
            "a member added that the ACL already lists, spelled differently",
            update_body(ACL, &["MEMBER-One"], &[]),
            "InvalidParameterCombinationException",
        ),
        (
            "an existing member added beside one that is not a member yet",
            update_body(ACL, &[OUTSIDER, INITIAL_MEMBERS[1]], &[]),
            "InvalidParameterCombinationException",
        ),
        (
            "an existing member added while another leaves",
            update_body(ACL, &[INITIAL_MEMBERS[1]], &[INITIAL_MEMBERS[0]]),
            "InvalidParameterCombinationException",
        ),
        (
            "one name on both sides of the change",
            update_body(ACL, &[OUTSIDER], &[OUTSIDER]),
            "InvalidParameterCombinationException",
        ),
        (
            "no change at all",
            json!({ "ACLName": ACL }),
            "InvalidParameterCombinationException",
        ),
        (
            "one name repeated in the added list",
            update_body(ACL, &[OUTSIDER, OUTSIDER], &[]),
            "InvalidParameterValueException",
        ),
        (
            "one name repeated in the removed list, in two spellings",
            update_body(ACL, &[], &[INITIAL_MEMBERS[0], "MEMBER-ONE"]),
            "InvalidParameterValueException",
        ),
        (
            "a member removed that the ACL does not list",
            update_body(ACL, &[], &[OUTSIDER]),
            "InvalidParameterValueException",
        ),
        (
            "an ACL that is in no state at all",
            update_body("no-such-acl", &[OUTSIDER], &[]),
            "ACLNotFoundFault",
        ),
    ];

    let expected_members: Vec<String> = {
        let mut names: Vec<String> = INITIAL_MEMBERS.iter().map(|n| n.to_string()).collect();
        names.sort();
        names
    };
    for (case, request, expected_code) in &cases {
        let (status, body) = call(&router, "UpdateACL", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the change to be refused with 400, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected error code {expected_code}, got {body}"
        );

        // A refused change persists NOTHING: not the membership, not the status, and
        // not a pending change for the clock to apply later.
        let unchanged = described_acl(&router, ACL, case).await;
        assert_eq!(
            members(&unchanged, case),
            expected_members,
            "case {case:?}: a refused change must leave the membership alone, got \
             {unchanged}"
        );
        assert_eq!(
            unchanged["Status"],
            json!("active"),
            "case {case:?}: a refused change must leave the status alone, got \
             {unchanged}"
        );
        assert_eq!(
            unchanged["PendingChanges"],
            Value::Null,
            "case {case:?}: a refused change must queue nothing, got {unchanged}"
        );
    }
}

/// The other half of the rule: a change that DOES agree with the current membership is
/// applied, so the classification above refuses mistakes rather than changes.
#[tokio::test]
async fn a_change_that_agrees_with_the_membership_is_applied() {
    let router = router();
    settled_acl(&router).await;

    // (case, added, removed, the membership afterwards) — each row starts from the
    // membership the previous row left behind.
    let cases: Vec<ChangeCase> = vec![
        (
            "a user that is not a member yet joins",
            vec![OUTSIDER],
            vec![],
            vec![INITIAL_MEMBERS[0], INITIAL_MEMBERS[1], OUTSIDER],
        ),
        (
            "a member leaves",
            vec![],
            vec![INITIAL_MEMBERS[0]],
            vec![INITIAL_MEMBERS[1], OUTSIDER],
        ),
        (
            "one joins while another leaves, named in another spelling",
            vec!["Member-One"],
            vec!["MEMBER-TWO"],
            vec![INITIAL_MEMBERS[0], OUTSIDER],
        ),
    ];

    for (case, to_add, to_remove, expected) in &cases {
        let updated = ok(
            &router,
            "UpdateACL",
            &update_body(ACL, to_add, to_remove),
            case,
        )
        .await;
        let acl = &updated["ACL"];
        let mut expected_members: Vec<String> =
            expected.iter().map(|name| name.to_string()).collect();
        expected_members.sort();
        assert_eq!(
            members(acl, case),
            expected_members,
            "case {case:?}: expected the membership {expected_members:?}, got {acl}"
        );
        assert_eq!(
            acl["Status"],
            json!("modifying"),
            "case {case:?}: an ACL applying a change reports modifying, got {acl}"
        );

        // The same membership is what a later read reports, from the one place it is
        // stored: the ACL. `DescribeUsers` derives `User$ACLNames` from it, so both
        // sides agree about every name the change moved.
        std::thread::sleep(PAST_SETTLE);
        let settled = described_acl(&router, ACL, case).await;
        assert_eq!(
            settled["Status"],
            json!("active"),
            "case {case:?}: expected the ACL to have settled, got {settled}"
        );
        assert_eq!(
            members(&settled, case),
            expected_members,
            "case {case:?}: the stored membership must round-trip, got {settled}"
        );
        for name in expected {
            let described = ok(
                &router,
                "DescribeUsers",
                &json!({ "UserName": name }),
                case,
            )
            .await;
            let acl_names = &described["Users"][0]["ACLNames"];
            assert!(
                acl_names
                    .as_array()
                    .is_some_and(|names| names.contains(&json!(ACL))),
                "case {case:?}: user {name:?} is a member, so its ACLNames must name \
                 {ACL:?}, got {acl_names}"
            );
        }
    }
}
