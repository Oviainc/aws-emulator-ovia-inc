// SPDX-License-Identifier: CC0-1.0

//! `UpdateUser`, end to end over the wire.
//!
//! Every case here drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.UpdateUser` through the multi-service router, so what is
//! asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * an update is a PATCH of the stored user: each member it supplies (`AccessString`,
//!   `AuthenticationMode`) replaces the stored one, an absent member leaves the stored
//!   value alone, and nothing else about the user (name, ARN, derived `ACLNames`, the
//!   other users in the store) moves;
//! * the update is answered from the SAME state `DescribeUsers` reads, so every patched
//!   member round-trips through the reader;
//! * a user AWS reports in a transient status settles: the update answers `modifying`
//!   and the user is `active` again once the clock owes it, and a second update before
//!   that is the modelled `InvalidUserStateFault`;
//! * `AuthenticationMode` is refused on an update exactly where it is refused on a
//!   create — the same values, the same modelled error codes — and a refused update
//!   leaves the stored user exactly as it was;
//! * the user every account ALREADY HAD is the service's, not the caller's: an update
//!   naming it is refused and it keeps its documented access string;
//! * a request naming no member to change is the modelled
//!   `InvalidParameterCombinationException`, and a name state does not hold is the
//!   modelled `UserNotFoundFault` — whatever else the request carries, the user is
//!   resolved before what is asked of it;
//! * a password is never echoed: only `Authentication$PasswordCount` is reported.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The suite-wide harness: ONE SigV4 request builder, ONE router constructor and ONE
// error-code reader for every suite in this crate, plus the user builders the create
// suite already defines, so both suites drive the same requests.
use crate::cluster_lifecycle::{call, error_code, ok, router};
use crate::user_lifecycle::{
    create_user, password_mode, password_of_length, FULL_ACCESS, MAX_PASSWORD_LEN, MIN_PASSWORD_LEN,
};

/// The region the suite-wide harness signs its requests for
/// (`crate::cluster_lifecycle::REGION`), which every ARN the emulator mints must carry.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The built-in user MemoryDB creates in every account
/// (`crate::state::DEFAULT_USER_NAME`): a user that is in state without any create, and
/// the service's rather than the caller's, so an update naming it is refused.
const DEFAULT_USER: &str = "default";

/// The access string the built-in user carries
/// (`crate::state::DEFAULT_USER_ACCESS_STRING`), which no update can move.
const DEFAULT_USER_ACCESS_STRING: &str = "on ~* &* +@all";

/// Longer than the span the service keeps a changed resource in its transient status
/// (`crate::state::SETTLE_AFTER`), so a call made after this sleep is owed the terminal
/// status. Slept on a thread rather than awaited, because the workspace's `tokio` is
/// declared without the `time` feature.
const PAST_SETTLE: Duration = Duration::from_millis(700);

/// The `Authentication` of a `password` user holding `count` passwords.
fn password_authentication(count: usize) -> Value {
    json!({ "Type": "password", "PasswordCount": count })
}

/// The single user `DescribeUsers` reports for `name`.
async fn describe_user(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let body = ok(router, "DescribeUsers", &json!({ "UserName": name }), case).await;
    body["Users"][0].clone()
}

/// One row of the patch table: what the user was created with, what the update asks
/// for, and the `AccessString` / `Authentication` the user must carry afterwards.
struct Patch {
    case: &'static str,
    created_access_string: &'static str,
    created_mode: Value,
    update: Value,
    expected_access_string: &'static str,
    expected_authentication: Value,
}

/// An update applies exactly the members it supplies and leaves every other one of the
/// stored user's alone — over rows that differ in which member is supplied, in the
/// authentication type, in the number of passwords and in the access string.
#[tokio::test]
async fn an_update_patches_only_the_members_it_supplies() {
    let one_password = password_of_length(MIN_PASSWORD_LEN);
    let other_password = password_of_length(MAX_PASSWORD_LEN);
    let rows = [
        Patch {
            case: "access string only",
            created_access_string: FULL_ACCESS,
            created_mode: password_mode(&[&one_password]),
            update: json!({ "AccessString": "on ~app:* +@read" }),
            // The service canonicalises the rule list, which always states the pub/sub
            // channel permission.
            expected_access_string: "on ~app:* resetchannels +@read",
            // Untouched: the request named no authentication mode.
            expected_authentication: password_authentication(1),
        },
        Patch {
            case: "authentication mode only, password to iam",
            created_access_string: "on ~metrics:* +get",
            created_mode: password_mode(&[&one_password]),
            update: json!({ "AuthenticationMode": { "Type": "iam" } }),
            // Untouched: the request named no access string, so the user keeps the
            // canonical form the create stored.
            expected_access_string: "on ~metrics:* resetchannels +get",
            expected_authentication: json!({ "Type": "iam" }),
        },
        Patch {
            case: "authentication mode only, iam to two passwords",
            created_access_string: "off ~* -@all",
            created_mode: json!({ "Type": "iam" }),
            update: json!({
                "AuthenticationMode": {
                    "Type": "password",
                    "Passwords": [one_password.clone(), other_password.clone()],
                },
            }),
            expected_access_string: "off ~* resetchannels -@all",
            expected_authentication: password_authentication(2),
        },
        Patch {
            case: "both members at once",
            created_access_string: FULL_ACCESS,
            created_mode: password_mode(&[&one_password, &other_password]),
            update: json!({
                "AccessString": "on ~cache:* &news:* +@write",
                "AuthenticationMode": { "Type": "password", "Passwords": [other_password.clone()] },
            }),
            expected_access_string: "on ~cache:* &news:* +@write",
            expected_authentication: password_authentication(1),
        },
        Patch {
            case: "a password count that does not change",
            created_access_string: FULL_ACCESS,
            created_mode: password_mode(&[&one_password]),
            update: json!({
                "AuthenticationMode": { "Type": "password", "Passwords": [other_password.clone()] },
            }),
            expected_access_string: FULL_ACCESS,
            expected_authentication: password_authentication(1),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let router = router();
        let case = row.case;
        let name = format!("patched-{index}");
        // A second user nobody updates, so an update that reaches past the name it was
        // given fails here.
        let bystander = create_user(&router, "bystander").await;
        ok(
            &router,
            "CreateUser",
            &json!({
                "UserName": name,
                "AccessString": row.created_access_string,
                "AuthenticationMode": row.created_mode,
            }),
            case,
        )
        .await;

        let mut request = row.update.clone();
        request["UserName"] = json!(name);
        let updated = ok(&router, "UpdateUser", &request, case).await;
        let user = &updated["User"];
        assert_eq!(
            user["Name"],
            json!(name),
            "case {case:?}: expected the updated user to be {name:?}, got {user}"
        );
        assert_eq!(
            user["AccessString"],
            json!(row.expected_access_string),
            "case {case:?}: expected AccessString {:?}, got {user}",
            row.expected_access_string
        );
        assert_eq!(
            user["Authentication"], row.expected_authentication,
            "case {case:?}: expected Authentication {}, got {user}",
            row.expected_authentication
        );
        assert_eq!(
            user["ARN"],
            json!(format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:user/{name}")),
            "case {case:?}: an update must not move the user's ARN, got {user}"
        );
        assert_eq!(
            user["Status"],
            json!("modifying"),
            "case {case:?}: an accepted update is not finished yet, got {user}"
        );
        // The passwords are secrets: nothing about them but the count is reported.
        let rendered = updated.to_string();
        for password in request["AuthenticationMode"]["Passwords"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            let password = password.as_str().unwrap_or_default();
            assert!(
                !rendered.contains(password),
                "case {case:?}: a password must never be echoed, got {updated}"
            );
        }

        // The round trip: the reader answers from the state the update wrote, and the
        // user is `active` again once the clock owes it.
        std::thread::sleep(PAST_SETTLE);
        let described = describe_user(&router, &name, case).await;
        assert_eq!(
            described["Status"],
            json!("active"),
            "case {case:?}: expected the user to have settled, got {described}"
        );
        assert_eq!(
            described["AccessString"],
            json!(row.expected_access_string),
            "case {case:?}: the describe must report what the update persisted, got \
             {described}"
        );
        assert_eq!(
            described["Authentication"], row.expected_authentication,
            "case {case:?}: the describe must report what the update persisted, got \
             {described}"
        );
        // And nothing else in the store moved.
        let other = describe_user(&router, "bystander", case).await;
        assert_eq!(
            other, bystander["User"],
            "case {case:?}: an update must touch only the user it names, got {other}"
        );
    }
}

/// `AuthenticationMode` is refused on an update exactly where it is refused on a
/// create — the same values, the same modelled codes — and a refused update leaves the
/// stored user exactly as it was.
#[tokio::test]
async fn an_update_refuses_every_authentication_mode_a_create_refuses() {
    let router = router();
    let password = password_of_length(MIN_PASSWORD_LEN);
    // (case, AuthenticationMode, expected error code or None when it must be accepted)
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        ("one password", password_mode(&[&password]), None),
        (
            "two passwords, the documented maximum",
            password_mode(&[&password, &format!("{password}x")]),
            None,
        ),
        ("an iam user", json!({ "Type": "iam" }), None),
        (
            "one password past the documented maximum",
            password_mode(&[&password, &format!("{password}x"), &format!("{password}y")]),
            Some("InvalidParameterValueException"),
        ),
        (
            "a password one character below the minimum length",
            password_mode(&[&password_of_length(MIN_PASSWORD_LEN - 1)]),
            Some("InvalidParameterValueException"),
        ),
        (
            "a password one character above the maximum length",
            password_mode(&[&password_of_length(MAX_PASSWORD_LEN + 1)]),
            Some("InvalidParameterValueException"),
        ),
        (
            "a password carrying punctuation the service accepts",
            password_mode(&[&format!("{password}@host")]),
            None,
        ),
        (
            "a password user with no passwords",
            json!({ "Type": "password" }),
            Some("InvalidParameterCombinationException"),
        ),
        (
            "an iam user carrying passwords",
            json!({ "Type": "iam", "Passwords": [password.clone()] }),
            Some("InvalidParameterCombinationException"),
        ),
        (
            "no authentication type at all",
            json!({}),
            Some("InvalidParameterValueException"),
        ),
    ];

    for (index, (case, mode, expected_code)) in rows.iter().enumerate() {
        // A fresh user per row, so the refusals are judged against a KNOWN stored user.
        let name = format!("mode-updated-{index}");
        let created = create_user(&router, &name).await;
        let (status, body) = call(
            &router,
            "UpdateUser",
            &json!({ "UserName": name, "AuthenticationMode": mode }),
        )
        .await;
        // The same mode through `CreateUser`: one helper decides both, so neither
        // operation can accept what the other refuses.
        let (create_status, create_body) = call(
            &router,
            "CreateUser",
            &json!({
                "UserName": format!("mode-created-{index}"),
                "AccessString": FULL_ACCESS,
                "AuthenticationMode": mode,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body)),
            (create_status, error_code(&create_body)),
            "case {case:?}: the create and the update must judge the same mode alike; \
             update answered {status} with {body}, create answered {create_status} with \
             {create_body}"
        );

        match expected_code {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update to succeed, got {status} with {body}"
                );
                let expected_count = mode["Passwords"].as_array().map(Vec::len);
                assert_eq!(
                    body["User"]["Authentication"]["PasswordCount"],
                    expected_count.map_or(Value::Null, |count| json!(count)),
                    "case {case:?}: expected PasswordCount {expected_count:?}, got {body}"
                );
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the update to be refused, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    *code,
                    "case {case:?}: expected error code {code}, got {body}"
                );
                // A refused update persists nothing: the user is as the create left it.
                let described = describe_user(&router, &name, case).await;
                assert_eq!(
                    described, created["User"],
                    "case {case:?}: a refused update must leave the stored user \
                     untouched, got {described}"
                );
            }
        }
    }
}

/// An update must name a user state holds, and that user must be in a valid state for
/// it: still `modifying` from an earlier update is the modelled `InvalidUserStateFault`,
/// and the same request is accepted once the clock has settled the user.
#[tokio::test]
async fn an_update_needs_an_existing_user_in_a_valid_state() {
    let router = router();
    let (status, body) = call(
        &router,
        "UpdateUser",
        &json!({ "UserName": "never-created", "AccessString": FULL_ACCESS }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"unknown user\": expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "UserNotFoundFault",
        "case \"unknown user\": expected the modelled not-found fault, got {body}"
    );

    // WHICH USER FIRST: a request naming a user state does not hold is answered
    // not-found whatever else it carries, including a mode that would be refused on a
    // user that does exist.
    let case = "unknown user, malformed mode";
    let (status, body) = call(
        &router,
        "UpdateUser",
        &json!({
            "UserName": "never-created",
            "AuthenticationMode": { "Type": "iam", "Passwords": [password_of_length(MIN_PASSWORD_LEN)] },
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "UserNotFoundFault".to_string()),
        "case {case:?}: the user must be resolved before what is asked of it, got \
         {status} with {body}"
    );

    // The built-in user the account already had is the SERVICE's, not the caller's: an
    // update naming it is refused whatever it asks for, and it keeps its documented
    // access string.
    let case = "the built-in user";
    for spelling in [DEFAULT_USER, "Default"] {
        let (status, body) = call(
            &router,
            "UpdateUser",
            &json!({ "UserName": spelling, "AccessString": "on ~* +@read" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: {spelling:?} must refuse the update, got {status} with {body}"
        );
    }
    let described = describe_user(&router, DEFAULT_USER, case).await;
    assert_eq!(
        described["AccessString"],
        json!(DEFAULT_USER_ACCESS_STRING),
        "case {case:?}: the refused update must leave the built-in user alone, got \
         {described}"
    );

    // A second update before the first has settled: the user is not in a valid state.
    let name = "settling-user";
    create_user(&router, name).await;
    let case = "an update that settles";
    ok(
        &router,
        "UpdateUser",
        &json!({ "UserName": name, "AccessString": "on ~* +@read" }),
        case,
    )
    .await;
    let case = "an update while modifying";
    let (status, body) = call(
        &router,
        "UpdateUser",
        &json!({ "UserName": name, "AccessString": FULL_ACCESS }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidUserStateFault",
        "case {case:?}: expected the modelled invalid-state fault, got {body}"
    );
    let described = describe_user(&router, name, case).await;
    assert_eq!(
        described["AccessString"],
        json!("on ~* resetchannels +@read"),
        "case {case:?}: the refused update must not have changed the stored user, got \
         {described}"
    );

    // Once the clock owes the user `active` again, the same request is accepted.
    let case = "an update after the settle";
    std::thread::sleep(PAST_SETTLE);
    let updated = ok(
        &router,
        "UpdateUser",
        &json!({ "UserName": name, "AccessString": FULL_ACCESS }),
        case,
    )
    .await;
    assert_eq!(
        updated["User"]["AccessString"],
        json!(FULL_ACCESS),
        "case {case:?}: expected the settled user to take the new access string, got \
         {updated}"
    );
}

/// A request naming no member to change asks for no modification at all, which is the
/// modelled `InvalidParameterCombinationException`, and it writes nothing: the user
/// keeps its stored members AND its `active` status.
#[tokio::test]
async fn an_update_that_asks_for_nothing_is_refused_and_writes_nothing() {
    let router = router();
    let created = create_user(&router, "unchanged").await;
    let case = "no members to change";
    let (status, body) = call(&router, "UpdateUser", &json!({ "UserName": "unchanged" })).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterCombinationException",
        "case {case:?}: expected the modelled invalid-combination fault, got {body}"
    );
    let described = describe_user(&router, "unchanged", case).await;
    assert_eq!(
        described, created["User"],
        "case {case:?}: a refused update must write nothing at all — not even the \
         transient status, got {described}"
    );
}

/// `User$ACLNames` stays DERIVED across an update: the update reports the ACLs that
/// currently list the user, and changing the user does not change its membership.
#[tokio::test]
async fn an_update_keeps_the_derived_acl_membership() {
    let router = router();
    create_user(&router, "member").await;
    for acl in ["acl-alpha", "acl-beta"] {
        ok(
            &router,
            "CreateACL",
            &json!({ "ACLName": acl, "UserNames": ["member"] }),
            acl,
        )
        .await;
    }
    let case = "membership across an update";
    let updated = ok(
        &router,
        "UpdateUser",
        &json!({ "UserName": "member", "AccessString": "on ~* +@read" }),
        case,
    )
    .await;
    assert_eq!(
        updated["User"]["ACLNames"],
        json!(["acl-alpha", "acl-beta"]),
        "case {case:?}: an update must report the membership state already holds, got \
         {updated}"
    );
    std::thread::sleep(PAST_SETTLE);
    let described = describe_user(&router, "member", case).await;
    assert_eq!(
        described["ACLNames"],
        json!(["acl-alpha", "acl-beta"]),
        "case {case:?}: the describe and the update must agree on the membership, got \
         {described}"
    );
}
