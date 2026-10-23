// SPDX-License-Identifier: CC0-1.0

//! `ACL$PendingChanges`, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rule under test, as an INVARIANT over a table of membership changes rather than
//! one recorded request: an ACL reports the updates being applied to it — the members
//! the accepted change adds and the members it removes — for exactly as long as it is
//! `modifying`, and reports no `PendingChanges` once it is `active` again. Each side of
//! the member is absent when the change names nobody on it, so a one-sided change
//! reports one side only.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{ok, router};

/// Longer than the service's `SETTLE_AFTER`, so the transition the clock owes an ACL is
/// certainly due by the time the test looks again. The emulator stores the due instant
/// and never sleeps; this blocks the test thread rather than awaiting, because the
/// workspace's `tokio` is declared without the `time` feature.
const PAST_SETTLE: Duration = Duration::from_millis(700);

/// `ACL$Status` while a change is being applied, and once it is done
/// (`smithy_query query="shape" shape="ACL"`: "Can be \"creating\", \"active\",
/// \"modifying\", \"deleting\"").
const MODIFYING: &str = "modifying";
const ACTIVE: &str = "active";

/// The users this suite's ACL draws its members from, spanning both authentication
/// modes a member can have, since the rule is about the membership change and not about
/// how a member authenticates.
const MEMBERS: &[(&str, bool)] = &[
    ("pending-one", false),
    ("pending-two", false),
    ("pending-iam", true),
    ("pending-four", false),
];

/// A `CreateUser` body for one of [`MEMBERS`].
fn create_user_body(name: &str, iam: bool) -> Value {
    if iam {
        json!({
            "UserName": name,
            "AccessString": "on ~* +@all",
            "AuthenticationMode": { "Type": "iam" },
        })
    } else {
        json!({
            "UserName": name,
            "AccessString": "on ~* +@all",
            "AuthenticationMode": {
                "Type": "password",
                "Passwords": ["a-sufficiently-long-password"],
            },
        })
    }
}

/// The `ACL` of a `DescribeACLs` answer for one ACL.
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

/// One step of the table: what the request asks for, what `PendingChanges` must then
/// report, and the membership the ACL holds once the change has settled.
struct Step {
    case: &'static str,
    to_add: &'static [&'static str],
    to_remove: &'static [&'static str],
    /// The `PendingChanges` the answer must carry, written out in full so an extra or
    /// missing side of the member fails the row.
    pending: fn() -> Value,
    members: &'static [&'static str],
}

/// The steps, differing along everything the rule generalises over: which side of the
/// change is named, how many names it carries, and the letter case they were written in
/// (a member name is a case-insensitive identifier, so the ACL holds the folded form).
const STEPS: &[Step] = &[
    Step {
        case: "one member added",
        to_add: &["pending-iam"],
        to_remove: &[],
        pending: || json!({ "UserNamesToAdd": ["pending-iam"] }),
        members: &["pending-one", "pending-iam"],
    },
    Step {
        case: "one member removed",
        to_add: &[],
        to_remove: &["pending-iam"],
        pending: || json!({ "UserNamesToRemove": ["pending-iam"] }),
        members: &["pending-one"],
    },
    Step {
        case: "two members added, named in mixed case",
        to_add: &["Pending-Two", "PENDING-FOUR"],
        to_remove: &[],
        pending: || json!({ "UserNamesToAdd": ["pending-four", "pending-two"] }),
        members: &["pending-one", "pending-four", "pending-two"],
    },
    Step {
        case: "one added and one removed at once",
        to_add: &["pending-iam"],
        to_remove: &["pending-four"],
        pending: || {
            json!({
                "UserNamesToAdd": ["pending-iam"],
                "UserNamesToRemove": ["pending-four"],
            })
        },
        members: &["pending-one", "pending-two", "pending-iam"],
    },
    Step {
        case: "two members removed",
        to_add: &[],
        to_remove: &["pending-two", "pending-iam"],
        pending: || json!({ "UserNamesToRemove": ["pending-iam", "pending-two"] }),
        members: &["pending-one"],
    },
];

/// An accepted membership change is reported as pending while the ACL is `modifying`,
/// and nothing is pending once the clock has settled it back to `active`.
#[tokio::test]
async fn an_accepted_membership_change_is_reported_while_it_settles() {
    let router = router();
    for (name, iam) in MEMBERS {
        ok(&router, "CreateUser", &create_user_body(name, *iam), name).await;
    }
    let acl_name = "pending-acl";
    let created = ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": acl_name, "UserNames": ["pending-one"] }),
        "setup",
    )
    .await;
    // A create is not a change being applied to an existing ACL: there is nothing in
    // flight on a membership the create itself established.
    assert!(
        created["ACL"].get("PendingChanges").is_none(),
        "case {:?}: a created ACL has no pending change, got {created}",
        "setup"
    );
    // Let the create settle, so each step below starts from an `active` ACL.
    std::thread::sleep(PAST_SETTLE);

    for step in STEPS {
        let case = step.case;
        let mut request = json!({ "ACLName": acl_name });
        if !step.to_add.is_empty() {
            request["UserNamesToAdd"] = json!(step.to_add);
        }
        if !step.to_remove.is_empty() {
            request["UserNamesToRemove"] = json!(step.to_remove);
        }
        let expected = (step.pending)();

        let updated = ok(&router, "UpdateACL", &request, case).await;
        let acl = &updated["ACL"];
        assert_eq!(
            acl["Status"],
            json!(MODIFYING),
            "case {case:?}: an accepted change leaves the ACL {MODIFYING}, got {updated}"
        );
        assert_eq!(
            acl["PendingChanges"], expected,
            "case {case:?}: expected PendingChanges {expected}, got {updated}"
        );

        // The same answer comes back out of state: while the ACL is still `modifying` it
        // reports that change, and the two members move together.
        let described = described_acl(&router, acl_name, case).await;
        if described["Status"] == json!(MODIFYING) {
            assert_eq!(
                described["PendingChanges"], expected,
                "case {case:?}: a describe must report the same pending change, got \
                 {described}"
            );
        } else {
            assert!(
                described.get("PendingChanges").is_none(),
                "case {case:?}: a settled ACL reports no pending change, got {described}"
            );
        }

        // Once the clock has settled the ACL, the change is no longer pending — and the
        // membership it asked for is what the ACL holds.
        std::thread::sleep(PAST_SETTLE);
        let settled = described_acl(&router, acl_name, case).await;
        assert_eq!(
            settled["Status"],
            json!(ACTIVE),
            "case {case:?}: the clock owes the ACL {ACTIVE}, got {settled}"
        );
        assert!(
            settled.get("PendingChanges").is_none(),
            "case {case:?}: a settled ACL reports no pending change, got {settled}"
        );
        let mut reported: Vec<String> = settled["UserNames"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: UserNames is a list, got {settled}"))
            .iter()
            .map(|name| name.as_str().unwrap_or_default().to_string())
            .collect();
        reported.sort();
        let mut members: Vec<String> = step.members.iter().map(|m| m.to_string()).collect();
        members.sort();
        assert_eq!(
            reported, members,
            "case {case:?}: expected the members {members:?}, got {reported:?}"
        );
    }
}

/// A refused change is not pending: an ACL whose update was refused reports the state it
/// already had, so nothing a client polls says a change is being applied.
#[tokio::test]
async fn a_refused_change_is_not_reported_as_pending() {
    let router = router();
    for (name, iam) in MEMBERS {
        ok(&router, "CreateUser", &create_user_body(name, *iam), name).await;
    }
    let acl_name = "refused-acl";
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": acl_name, "UserNames": ["pending-one"] }),
        "setup",
    )
    .await;
    std::thread::sleep(PAST_SETTLE);

    // One row per refusal rule UpdateACL enforces, with the status each is refused
    // with, each naming members this ACL could otherwise have taken.
    let refusals: &[(&str, Value, u16)] = &[
        ("nothing asked for", json!({ "ACLName": acl_name }), 400),
        (
            "a member added that is already one",
            json!({ "ACLName": acl_name, "UserNamesToAdd": ["pending-one"] }),
            400,
        ),
        (
            "a member removed that is not one",
            json!({ "ACLName": acl_name, "UserNamesToRemove": ["pending-two"] }),
            400,
        ),
        (
            "one name added and removed at once",
            json!({
                "ACLName": acl_name,
                "UserNamesToAdd": ["pending-two"],
                "UserNamesToRemove": ["Pending-Two"],
            }),
            400,
        ),
    ];
    for (case, request, expected_status) in refusals {
        let (status, body) = crate::cluster_lifecycle::call(&router, "UpdateACL", request).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected the change to be refused with {expected_status}, got \
             {status} with {body}"
        );
        let described = described_acl(&router, acl_name, case).await;
        assert_eq!(
            described["Status"],
            json!(ACTIVE),
            "case {case:?}: a refused change leaves the ACL {ACTIVE}, got {described}"
        );
        assert!(
            described.get("PendingChanges").is_none(),
            "case {case:?}: a refused change is not pending, got {described}"
        );
        assert_eq!(
            described["UserNames"],
            json!(["pending-one"]),
            "case {case:?}: a refused change leaves the membership alone, got {described}"
        );
    }
}
