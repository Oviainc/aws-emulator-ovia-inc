// SPDX-License-Identifier: CC0-1.0

//! How a refused ACL-membership write is CLASSIFIED and WORDED, end to end over the
//! wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! status, error code and message and not a handler called directly.
//!
//! The rule under test, over a SET of requests rather than one recorded one: a
//! membership write this service cannot apply is refused by WHAT IS WRONG with it, in
//! the service's own sentence for that mistake, and the refusal is the same whichever
//! write path (`CreateACL$UserNames`, `UpdateACL$UserNamesToAdd`,
//! `UpdateACL$UserNamesToRemove`) and whichever spelling carried it:
//!
//! * a name a LIST repeats is a bad list value — `InvalidParameterValueException` (400)
//!   with fixed text that echoes no name at all, on both write paths;
//! * a removal naming an existing user that this ACL does not list is a bad parameter
//!   value too — `InvalidParameterValueException` (400), naming the user and the ACL;
//! * an addition naming a user the ACL ALREADY lists is a delta that cannot hold
//!   against the membership it would be applied to — `InvalidParameterCombinationException`
//!   (400), naming the user and the ACL on their own, with no noun before either;
//! * a change that names neither list asks for nothing —
//!   `InvalidParameterCombinationException` (400) with fixed text that names no wire
//!   member;
//! * only a name NO user exists for is the absent-user fault — `UserNotFoundFault`,
//!   whichever list carried it (at the status this service's `awsJson1_1` wire answers
//!   every content fault with — see `fault_http_status`).
//!
//! The variable parts of every message come from the request and from state (the FOLDED
//! resource names the service stores and reports), never from a value the fixed text has
//! no slot for; and a refusal leaves the ACL exactly as it was.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::user_lifecycle::create_user;

/// Longer than the service's `SETTLE_AFTER`, so the transition the clock owes the ACL is
/// certainly due by the time the test looks again. The emulator stores the due instant
/// and never sleeps; this blocks the test thread rather than awaiting, because the
/// workspace's `tokio` is declared without the `time` feature.
const PAST_SETTLE: Duration = Duration::from_millis(700);

/// The ACL every case works on, the member it starts with, and two users that exist but
/// belong to no ACL — so a change naming one of them is about the MEMBERSHIP rather than
/// about whether the user exists.
const ACL: &str = "wording-acl";
const MEMBER: &str = "wording-member";
const OUTSIDER: &str = "wording-outsider";
const SECOND_OUTSIDER: &str = "wording-outsider-two";

/// A name no user exists for, so a list carrying it names no member at all.
const ABSENT: &str = "wording-absent";

// The sentences the service answers each membership mistake with. Fixed text, taken
// from the recorded refusals of this service's ACL-membership writes; the names in a
// template below are this suite's own resources, substituted where the sentence has a
// slot for them.
/// A list that names one user twice: the mistake is stated and no name is echoed.
const DUPLICATE_MEMBER: &str = "Duplicate user name specified.";
/// A change that names neither membership list: the sentence asks for the change and
/// names no wire member.
const NO_CHANGE: &str = "Please specify the list of user names to be added or removed.";

/// The sentence a removal naming a user that is not in this ACL is refused with.
fn not_member(user_name: &str, acl_name: &str) -> String {
    format!("User {user_name} is not a member of ACL {acl_name}.")
}

/// The sentence an addition naming a user the ACL already lists is refused with: neither
/// value is prefixed with a noun.
fn already_member(user_name: &str, acl_name: &str) -> String {
    format!("{user_name} is already a member of {acl_name}.")
}

/// The sentence every operation of this service states an absent user with.
fn user_not_found(user_name: &str) -> String {
    format!("User {user_name} not found.")
}

/// The `Message` of a failure body, under either spelling of the member.
fn error_message(body: &Value, case: &str) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {body}"))
        .to_string()
}

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

/// The users every case draws on, plus the ACL holding [`MEMBER`], waited out to
/// `active` so it can take a change.
async fn settled_acl(router: &MultiServiceRouter) {
    for name in [MEMBER, OUTSIDER, SECOND_OUTSIDER] {
        create_user(router, name).await;
    }
    ok(
        router,
        "CreateACL",
        &json!({ "ACLName": ACL, "UserNames": [MEMBER] }),
        "setup",
    )
    .await;
    std::thread::sleep(PAST_SETTLE);
    let described = ok(router, "DescribeACLs", &json!({ "ACLName": ACL }), "setup").await;
    assert_eq!(
        described["ACLs"][0]["Status"],
        json!("active"),
        "case \"setup\": the ACL must be active before a change is applied, got \
         {described}"
    );
}

/// One row: the case name, the operation, the request, and the status, code and message
/// the service answers it with.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    status: u16,
    code: &'static str,
    message: String,
}

/// Every membership mistake is reported by the fault for THAT mistake, in the service's
/// own sentence for it, and leaves the ACL exactly as it was.
#[tokio::test]
async fn a_refused_membership_write_states_the_mistake_it_made() {
    let router = router();
    settled_acl(&router).await;

    // The rows differ in which list carried the offending name, how many names the list
    // carries and where in it the offending one sits, how the names are spelled, which
    // write path took them, and whether the name resolves to a user at all.
    let rows: Vec<Row> = vec![
        Row {
            case: "a member added that the ACL already lists",
            operation: "UpdateACL",
            body: update_body(ACL, &[MEMBER], &[]),
            status: 400,
            code: "InvalidParameterCombinationException",
            message: already_member(MEMBER, ACL),
        },
        Row {
            case: "the same addition, both names spelled in another case",
            operation: "UpdateACL",
            body: update_body("Wording-ACL", &["WORDING-Member"], &[]),
            status: 400,
            code: "InvalidParameterCombinationException",
            // The names come from state, which holds them folded, so the sentence reads
            // the same whichever spelling the caller sent.
            message: already_member(MEMBER, ACL),
        },
        Row {
            case: "an existing member added beside a user that is not a member yet",
            operation: "UpdateACL",
            body: update_body(ACL, &[OUTSIDER, MEMBER], &[]),
            status: 400,
            code: "InvalidParameterCombinationException",
            message: already_member(MEMBER, ACL),
        },
        Row {
            case: "a removal naming an existing user the ACL does not list",
            operation: "UpdateACL",
            body: update_body(ACL, &[], &[OUTSIDER]),
            status: 400,
            code: "InvalidParameterValueException",
            message: not_member(OUTSIDER, ACL),
        },
        Row {
            case: "that removal spelled in another case",
            operation: "UpdateACL",
            body: update_body("WORDING-acl", &[], &["Wording-Outsider"]),
            status: 400,
            code: "InvalidParameterValueException",
            message: not_member(OUTSIDER, ACL),
        },
        Row {
            case: "a removal naming a member beside a user the ACL does not list",
            operation: "UpdateACL",
            body: update_body(ACL, &[], &[MEMBER, SECOND_OUTSIDER]),
            status: 400,
            code: "InvalidParameterValueException",
            message: not_member(SECOND_OUTSIDER, ACL),
        },
        Row {
            case: "a change that names neither membership list",
            operation: "UpdateACL",
            body: json!({ "ACLName": ACL }),
            status: 400,
            code: "InvalidParameterCombinationException",
            message: NO_CHANGE.to_string(),
        },
        Row {
            case: "one name repeated in the added list",
            operation: "UpdateACL",
            body: update_body(ACL, &[OUTSIDER, OUTSIDER], &[]),
            status: 400,
            code: "InvalidParameterValueException",
            message: DUPLICATE_MEMBER.to_string(),
        },
        Row {
            case: "one name repeated in the removed list, in two spellings",
            operation: "UpdateACL",
            body: update_body(ACL, &[], &[MEMBER, "WORDING-MEMBER"]),
            status: 400,
            code: "InvalidParameterValueException",
            message: DUPLICATE_MEMBER.to_string(),
        },
        Row {
            case: "a repeat that is not adjacent, in a longer added list",
            operation: "UpdateACL",
            body: update_body(ACL, &[OUTSIDER, SECOND_OUTSIDER, OUTSIDER], &[]),
            status: 400,
            code: "InvalidParameterValueException",
            message: DUPLICATE_MEMBER.to_string(),
        },
        Row {
            case: "a create whose membership repeats a name",
            operation: "CreateACL",
            body: json!({ "ACLName": "repeat-acl", "UserNames": [OUTSIDER, OUTSIDER] }),
            status: 400,
            code: "InvalidParameterValueException",
            message: DUPLICATE_MEMBER.to_string(),
        },
        Row {
            case: "a create whose longer membership repeats a name in two spellings",
            operation: "CreateACL",
            body: json!({
                "ACLName": "repeat-acl-two",
                "UserNames": [OUTSIDER, SECOND_OUTSIDER, "WORDING-Outsider"],
            }),
            status: 400,
            code: "InvalidParameterValueException",
            message: DUPLICATE_MEMBER.to_string(),
        },
        // The absent-user fault is what a name NO user exists for still gets, on either
        // list and on either write path: that is the one mistake the membership rules
        // above do not cover.
        Row {
            case: "an addition naming a user that is in no state at all",
            operation: "UpdateACL",
            body: update_body(ACL, &[ABSENT], &[]),
            status: 400,
            code: "UserNotFoundFault",
            message: user_not_found(ABSENT),
        },
        Row {
            case: "a removal naming a user that is in no state at all",
            operation: "UpdateACL",
            body: update_body(ACL, &[], &[ABSENT]),
            status: 400,
            code: "UserNotFoundFault",
            message: user_not_found(ABSENT),
        },
        Row {
            case: "a create whose membership names a user that is in no state at all",
            operation: "CreateACL",
            body: json!({ "ACLName": "absent-member-acl", "UserNames": [ABSENT] }),
            status: 400,
            code: "UserNotFoundFault",
            message: user_not_found(ABSENT),
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, body) = call(&router, row.operation, &row.body).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected error code {}, got {body}",
            row.code
        );
        let message = error_message(&body, case);
        assert_eq!(
            message, row.message,
            "case {case:?}: expected the message {:?}, got {message:?}",
            row.message
        );
    }

    // A refused create leaves no ACL behind, so the name it asked for is still in no
    // state at all.
    for name in ["repeat-acl", "repeat-acl-two", "absent-member-acl"] {
        let (status, body) = call(&router, "DescribeACLs", &json!({ "ACLName": name })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ACLNotFoundFault"),
            "case {name:?}: a refused create must leave no ACL behind, got {status} \
             with {body}"
        );
    }

    // Every refusal above left the ACL exactly as it was, so the change that DOES hold
    // against that membership still lands afterwards.
    let unchanged = ok(&router, "DescribeACLs", &json!({ "ACLName": ACL }), "after").await;
    assert_eq!(
        (
            unchanged["ACLs"][0]["Status"].clone(),
            unchanged["ACLs"][0]["UserNames"].clone()
        ),
        (json!("active"), json!([MEMBER])),
        "case \"after\": the refusals must leave the ACL alone, got {unchanged}"
    );
    let updated = ok(
        &router,
        "UpdateACL",
        &update_body(ACL, &[OUTSIDER], &[]),
        "after",
    )
    .await;
    assert_eq!(
        updated["ACL"]["UserNames"],
        json!([MEMBER, OUTSIDER]),
        "case \"after\": a change that holds must be applied, got {updated}"
    );
    std::thread::sleep(PAST_SETTLE);
    let settled = ok(&router, "DescribeACLs", &json!({ "ACLName": ACL }), "after").await;
    assert_eq!(
        (
            settled["ACLs"][0]["Status"].clone(),
            settled["ACLs"][0]["UserNames"].clone()
        ),
        (json!("active"), json!([MEMBER, OUTSIDER])),
        "case \"after\": the applied membership must round-trip, got {settled}"
    );
}
