// SPDX-License-Identifier: CC0-1.0

//! The CANONICAL form of `User$AccessString`, and the `default` user's ACL membership,
//! end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * MemoryDB stores the access string in the canonical form of the Redis OSS / Valkey
//!   ACL rule list, which always states BOTH permissions the engine holds a default
//!   for: the pub/sub channel permission (`resetchannels` when the string names none)
//!   and the command permission BASELINE — the command section opens with the full-set
//!   rule (`+@all`/`allcommands` or `-@all`/`nocommands`) that the command and category
//!   rules after it modify, so a string whose command rules do not begin with one gets
//!   the engine's `-@all`. Every echo path of a stored user reports that one stored
//!   form: `CreateUser`, `DescribeUsers`, `UpdateUser` and `DeleteUser`.
//! * The canonical form is a FIXED POINT: sending a string that is already canonical
//!   stores it unchanged, so a client that re-applies what it read back sees no drift.
//! * The built-in `default` user is a member of the built-in `open-access` ACL and of no
//!   other, so both write paths of an ACL's membership refuse to put it in another ACL —
//!   and an ACL created without `UserNames` has an EMPTY membership, which is what every
//!   path reporting that ACL says.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::state::{
    DEFAULT_CHANNEL_RULE, DEFAULT_COMMAND_RULE, DEFAULT_USER_ACCESS_STRING, DEFAULT_USER_NAME,
    MIN_PASSWORD_LEN, OPEN_ACCESS_ACL_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

// The suite-wide harness: ONE SigV4 request builder, ONE router constructor and ONE
// error-code reader for every suite in this crate.
use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::user_lifecycle::{create_user, password_mode, password_of_length};

/// Longer than the service's `SETTLE_AFTER`, so a transition the clock owes a user or an
/// ACL is certainly due by the time the test looks again. The emulator stores the due
/// instant and never sleeps; this blocks the test thread rather than awaiting, because
/// the workspace's `tokio` is declared without the `time` feature.
const PAST_SETTLE: Duration = Duration::from_millis(700);

/// The error message AWS answers an attempt to put the built-in user in another ACL
/// with, as the recorded `CreateACL` refusal carries it.
const DEFAULT_USER_MEMBERSHIP_MESSAGE: &str = "default user can't be added to another ACL.";

/// One row of the canonical-form table: the access string as the request sends it, and
/// the form the service stores and reports for it.
struct Canonical {
    sent: &'static str,
    canonical: &'static str,
}

/// The table: the rows differ in whether the string states its channel permission, in
/// which keyword it states it with, in whether its command section opens with a full-set
/// rule and in which spelling of one, in how many command rules follow it, in how many
/// key patterns precede it, in whether the string carries any command rule at all, and
/// in how it is spaced. Every rule named here is of the Redis OSS / Valkey ACL syntax
/// `User$AccessString` carries (`smithy_query(query="doc",shape="com.amazonaws.memorydb#User",member="AccessString")`
/// types it as a plain string: the syntax is the engine's).
fn canonical_forms() -> Vec<Canonical> {
    vec![
        // A granted category with no baseline stated: the engine's `-@all` opens the
        // command section.
        Canonical {
            sent: "on ~app:* resetchannels +@read",
            canonical: "on ~app:* resetchannels -@all +@read",
        },
        // Neither default stated, and the key pattern spelled with a second colon.
        Canonical {
            sent: "on ~app::* +@read",
            canonical: "on ~app::* resetchannels -@all +@read",
        },
        // The command section opens with a full-set rule already: only the channel
        // default is missing.
        Canonical {
            sent: "on ~* +@all",
            canonical: "on ~* resetchannels +@all",
        },
        // Both permissions stated: returned exactly as sent.
        Canonical {
            sent: "on ~* &* +@all",
            canonical: "on ~* &* +@all",
        },
        // Several key patterns and several granted commands: ONE baseline, in front of
        // the first command rule.
        Canonical {
            sent: "off ~key1 ~key2 +get +set",
            canonical: "off ~key1 ~key2 resetchannels -@all +get +set",
        },
        // A denied category is a modification of the baseline too, not a baseline.
        Canonical {
            sent: "on ~* &news:* -@admin",
            canonical: "on ~* &news:* -@all -@admin",
        },
        // No command rule at all: both defaults go at the end, in the canonical order.
        Canonical {
            sent: "on ~*",
            canonical: "on ~* resetchannels -@all",
        },
        // The keyword spellings of the two defaults are the defaults: nothing is added.
        Canonical {
            sent: "on ~* allchannels nocommands",
            canonical: "on ~* allchannels nocommands",
        },
        // `allcommands` is the keyword spelling of `+@all`, so it is a baseline as well.
        Canonical {
            sent: "off ~app:* resetchannels allcommands",
            canonical: "off ~app:* resetchannels allcommands",
        },
        // A baseline the caller stated itself, with the grant it modifies after it.
        Canonical {
            sent: "on ~* &* -@all +@write",
            canonical: "on ~* &* -@all +@write",
        },
        // Extra whitespace is not part of a rule list.
        Canonical {
            sent: "on   ~metrics:*    +@read   +mget",
            canonical: "on ~metrics:* resetchannels -@all +@read +mget",
        },
    ]
}

/// `CreateUser` with `name`, one password and `access_string`.
fn create_user_body(name: &str, access_string: &str) -> Value {
    json!({
        "UserName": name,
        "AccessString": access_string,
        "AuthenticationMode": password_mode(&[&password_of_length(MIN_PASSWORD_LEN)]),
    })
}

/// The single user `DescribeUsers` reports for `name`.
async fn described_user(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let body = ok(router, "DescribeUsers", &json!({ "UserName": name }), case).await;
    body["Users"][0].clone()
}

/// Every echo path of a stored user reports the one canonical access string: the create
/// that stored it, the describe that reads it, the update that replaced it and the
/// delete that answers with the user as it last was.
#[tokio::test]
async fn every_user_echo_path_reports_the_canonical_access_string() {
    let router = router();
    let rows = canonical_forms();

    // Create one user per row, and read it straight back.
    for (index, row) in rows.iter().enumerate() {
        let case = row.sent;
        let name = format!("canonical-{index}");
        let created = ok(&router, "CreateUser", &create_user_body(&name, case), case).await;
        assert_eq!(
            created["User"]["AccessString"],
            json!(row.canonical),
            "case {case:?}: expected CreateUser to store {:?}, got {created}",
            row.canonical
        );
        let described = described_user(&router, &name, case).await;
        assert_eq!(
            described["AccessString"],
            json!(row.canonical),
            "case {case:?}: expected DescribeUsers to report {:?}, got {described}",
            row.canonical
        );
    }

    // Update each user to the NEXT row's string, so the canonicalisation runs on the
    // update path over the whole table too, and read it back again.
    for (index, row) in rows.iter().enumerate() {
        let next = &rows[(index + 1) % rows.len()];
        let case = format!("{} -> {}", row.sent, next.sent);
        let name = format!("canonical-{index}");
        let updated = ok(
            &router,
            "UpdateUser",
            &json!({ "UserName": name, "AccessString": next.sent }),
            &case,
        )
        .await;
        assert_eq!(
            updated["User"]["AccessString"],
            json!(next.canonical),
            "case {case:?}: expected UpdateUser to store {:?}, got {updated}",
            next.canonical
        );
        let described = described_user(&router, &name, &case).await;
        assert_eq!(
            described["AccessString"],
            json!(next.canonical),
            "case {case:?}: expected DescribeUsers to report {:?}, got {described}",
            next.canonical
        );
    }

    // The clock owes every updated user `active` again; once it is due, the delete
    // answers with the user as it last was — carrying the same stored form.
    std::thread::sleep(PAST_SETTLE);
    for (index, row) in rows.iter().enumerate() {
        let next = &rows[(index + 1) % rows.len()];
        let case = format!("delete {}", row.sent);
        let name = format!("canonical-{index}");
        let deleted = ok(&router, "DeleteUser", &json!({ "UserName": name }), &case).await;
        assert_eq!(
            deleted["User"]["AccessString"],
            json!(next.canonical),
            "case {case:?}: expected DeleteUser to answer with {:?}, got {deleted}",
            next.canonical
        );
    }
}

/// The canonical form is a FIXED POINT: a request sending a string the service itself
/// reported stores it unchanged, so a client re-applying what it read sees no drift —
/// and the access string of the built-in user is already canonical.
#[tokio::test]
async fn the_canonical_form_is_a_fixed_point() {
    let router = router();
    for (index, row) in canonical_forms().iter().enumerate() {
        let case = row.canonical;
        let name = format!("fixed-point-{index}");
        let created = ok(&router, "CreateUser", &create_user_body(&name, case), case).await;
        assert_eq!(
            created["User"]["AccessString"],
            json!(row.canonical),
            "case {case:?}: an already canonical string must be stored unchanged, got \
             {created}"
        );
        // And the canonical form states both defaults, whichever rules it states them
        // with: a form carrying neither keyword could not be a fixed point.
        let rules: Vec<&str> = row.canonical.split(' ').collect();
        assert!(
            rules
                .iter()
                .any(|rule| rule.starts_with('&') || *rule == DEFAULT_CHANNEL_RULE
                    || *rule == "allchannels"),
            "case {case:?}: the canonical form states the channel permission, got {rules:?}"
        );
        assert!(
            rules.iter().any(|rule| *rule == DEFAULT_COMMAND_RULE
                || *rule == "+@all"
                || *rule == "allcommands"
                || *rule == "nocommands"),
            "case {case:?}: the canonical form states the command baseline, got {rules:?}"
        );
    }

    let case = DEFAULT_USER_NAME;
    let builtin = described_user(&router, DEFAULT_USER_NAME, case).await;
    assert_eq!(
        builtin["AccessString"],
        json!(DEFAULT_USER_ACCESS_STRING),
        "case {case:?}: the built-in user's access string is already canonical, got \
         {builtin}"
    );
}

/// An ACL created without `UserNames` has an EMPTY membership, and every path reporting
/// that ACL says so — the built-in `default` user is a member of the built-in
/// `open-access` ACL alone.
#[tokio::test]
async fn an_acl_created_without_members_has_an_empty_membership() {
    let router = router();
    // One row per way of asking for an ACL with no members of its own: the member is
    // absent entirely, and (the same request again under another name) absent beside
    // the tags a create may carry.
    let cases: &[(&str, Value)] = &[
        ("no UserNames", json!({ "ACLName": "empty-acl" })),
        (
            "no UserNames, with tags",
            json!({
                "ACLName": "tagged-empty-acl",
                "Tags": [{ "Key": "team", "Value": "platform" }],
            }),
        ),
    ];
    for (case, body) in cases {
        let created = ok(&router, "CreateACL", body, case).await;
        assert_eq!(
            created["ACL"]["UserNames"],
            json!([]),
            "case {case:?}: a create naming no member has an empty membership, got \
             {created}"
        );
        let name = body["ACLName"].as_str().expect("the row names its ACL");
        let described = ok(&router, "DescribeACLs", &json!({ "ACLName": name }), case).await;
        assert_eq!(
            described["ACLs"][0]["UserNames"],
            json!([]),
            "case {case:?}: the describe must report the stored membership, got {described}"
        );
    }

    // The built-in ACL is the one that holds the built-in user, and it keeps it.
    let builtin = ok(
        &router,
        "DescribeACLs",
        &json!({ "ACLName": OPEN_ACCESS_ACL_NAME }),
        OPEN_ACCESS_ACL_NAME,
    )
    .await;
    assert_eq!(
        builtin["ACLs"][0]["UserNames"],
        json!([DEFAULT_USER_NAME]),
        "case {OPEN_ACCESS_ACL_NAME:?}: the built-in ACL holds the built-in user, got \
         {builtin}"
    );

    // Once the clock owes each ACL `active`, the delete answers with the ACL as it last
    // was — the same empty membership.
    std::thread::sleep(PAST_SETTLE);
    for (case, body) in cases {
        let name = body["ACLName"].as_str().expect("the row names its ACL");
        let deleted = ok(&router, "DeleteACL", &json!({ "ACLName": name }), case).await;
        assert_eq!(
            deleted["ACL"]["UserNames"],
            json!([]),
            "case {case:?}: the delete must answer with the stored membership, got \
             {deleted}"
        );
    }
}

/// Neither write path of an ACL's membership may put the built-in `default` user in an
/// ACL other than the built-in one: each is refused with the modelled
/// `InvalidParameterValueException`, and nothing is written.
#[tokio::test]
async fn the_builtin_user_cannot_be_made_a_member_of_another_acl() {
    let router = router();
    create_user(&router, "acl-member").await;

    // One row per membership naming the built-in user: alone, beside another member,
    // and in each spelling a folded user name takes (a MemoryDB name is a
    // case-insensitive identifier, so a differently spelled `default` is the same user).
    let memberships: &[(&str, Value)] = &[
        ("the built-in user alone", json!([DEFAULT_USER_NAME])),
        ("beside another member", json!(["acl-member", "default"])),
        ("before another member", json!(["default", "acl-member"])),
        ("spelled in title case", json!(["Default"])),
        ("spelled in upper case", json!(["DEFAULT"])),
    ];
    for (index, (case, user_names)) in memberships.iter().enumerate() {
        let name = format!("holds-default-{index}");
        let (status, body) = call(
            &router,
            "CreateACL",
            &json!({ "ACLName": name, "UserNames": user_names }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected the modelled invalid-value error, got {status} with \
             {body}"
        );
        assert_eq!(
            body["message"],
            json!(DEFAULT_USER_MEMBERSHIP_MESSAGE),
            "case {case:?}: expected the message {DEFAULT_USER_MEMBERSHIP_MESSAGE:?}, got \
             {body}"
        );
        // Nothing was written: the ACL the refused create named is in no state at all.
        // (The HTTP status a not-found fault answers with is its own issue's rule; what
        // this row asserts is that the refused create persisted nothing.)
        let (status, body) = call(&router, "DescribeACLs", &json!({ "ACLName": name })).await;
        assert_eq!(
            error_code(&body),
            "ACLNotFoundFault",
            "case {case:?}: a refused create must persist nothing, got {status} with {body}"
        );
    }

    // The same rule on the update path, against an ACL that does exist.
    let acl = "adds-default";
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": acl, "UserNames": ["acl-member"] }),
        acl,
    )
    .await;
    for (case, user_names) in memberships {
        let (status, body) = call(
            &router,
            "UpdateACL",
            &json!({ "ACLName": acl, "UserNamesToAdd": user_names }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected the modelled invalid-value error, got {status} with \
             {body}"
        );
        assert_eq!(
            body["message"],
            json!(DEFAULT_USER_MEMBERSHIP_MESSAGE),
            "case {case:?}: expected the message {DEFAULT_USER_MEMBERSHIP_MESSAGE:?}, got \
             {body}"
        );
    }
    let described = ok(
        &router,
        "DescribeACLs",
        &json!({ "ACLName": acl }),
        "after the refusals",
    )
    .await;
    assert_eq!(
        described["ACLs"][0]["UserNames"],
        json!(["acl-member"]),
        "case {:?}: a refused change must leave the membership alone, got {described}",
        "after the refusals"
    );
}
