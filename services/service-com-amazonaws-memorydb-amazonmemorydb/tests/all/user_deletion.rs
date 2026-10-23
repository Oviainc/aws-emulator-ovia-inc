// SPDX-License-Identifier: CC0-1.0

//! Deleting a MemoryDB user, end to end over the wire.
//!
//! Every case here drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router, so what
//! is asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rule under test, over a SET of inputs rather than one recorded request
//! (`DeleteUser`: "Deletes a user. The user will be removed from all ACLs and in turn
//! removed from all clusters."):
//!
//! * a delete takes the named user out of the store AND out of the `UserNames` of
//!   every ACL that listed it, however many ACLs that is — none, one or several;
//! * the user every account ALREADY HAD is the service's, not the caller's: a delete
//!   naming it is refused with the modelled invalid-value error and it stays;
//! * it answers with that user as the caller last had it, in the terminal `deleting`
//!   status, carrying the membership it held up to the call;
//! * it touches no other user and no other ACL member, and a deleted user never comes
//!   back;
//! * naming a user that is not in state is the modelled not-found fault, and such a
//!   refusal changes nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The suite-wide harness: ONE SigV4 request builder, ONE router constructor and ONE
// error-code reader for every suite in this crate.
use crate::cluster_lifecycle::{call, error_code, ok, router};
// The user-suite helpers, so a user is created here exactly as it is there.
use crate::user_lifecycle::{create_user, user_names};

/// The region the suite-wide harness signs its requests for
/// (`crate::cluster_lifecycle::REGION`), which every ARN the emulator mints carries.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The built-in user MemoryDB creates in every account
/// (`crate::state::DEFAULT_USER_NAME`), which a delete must refuse, and the built-in
/// ACL that holds it (`crate::state::OPEN_ACCESS_ACL_NAME`), which no caller's delete
/// touches either.
const DEFAULT_USER: &str = "default";
const OPEN_ACCESS_ACL: &str = "open-access";

/// A user that belongs to every ACL below and is deleted by nothing, so each case can
/// assert that a delete left the OTHER members of an ACL exactly where they were.
const BYSTANDER: &str = "bystander";

/// The `UserNames` of one ACL in a `DescribeACLs` body.
fn acl_members(acl: &Value) -> Vec<String> {
    acl["UserNames"]
        .as_array()
        .unwrap_or_else(|| panic!("expected UserNames in {acl}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("expected a user name in {acl}"))
                .to_string()
        })
        .collect()
}

/// Every ACL in state, as `(ACLName, UserNames)` pairs in name order. The built-in
/// `open-access` ACL is left out: no caller's delete can reach it, because the only
/// user it holds is the built-in one a delete refuses.
async fn acl_membership(router: &MultiServiceRouter, case: &str) -> Vec<(String, Vec<String>)> {
    let listed = ok(router, "DescribeACLs", &json!({}), case).await;
    listed["ACLs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ACLs in {listed}"))
        .iter()
        .filter(|acl| acl["Name"] != json!(OPEN_ACCESS_ACL))
        .map(|acl| {
            (
                acl["Name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected Name in {acl}"))
                    .to_string(),
                acl_members(acl),
            )
        })
        .collect()
}

/// A delete removes the user from the store and from every ACL that listed it, for a
/// user that belongs to no ACL, to one and to several — and refuses the built-in user
/// the account already had.
#[tokio::test]
async fn deleting_a_user_removes_it_from_state_and_from_every_acl() {
    let router = router();
    // The ACLs this case builds, in name order.
    let acls = ["acl-one", "acl-three", "acl-two"];
    // (user, the ACLs that list it) — the rows differ in HOW MANY ACLs name the user
    // (none, one, every one of them).
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("in-no-acl", vec![]),
        ("in-one-acl", vec!["acl-one"]),
        ("in-every-acl", vec!["acl-one", "acl-three", "acl-two"]),
    ];

    create_user(&router, BYSTANDER).await;
    for (name, _) in &cases {
        create_user(&router, name).await;
    }
    for acl in acls {
        let mut members: Vec<&str> = cases
            .iter()
            .filter(|(_, member_of)| member_of.contains(&acl))
            .map(|(name, _)| *name)
            .collect();
        members.push(BYSTANDER);
        ok(
            &router,
            "CreateACL",
            &json!({ "ACLName": acl, "UserNames": members }),
            acl,
        )
        .await;
    }

    for (name, member_of) in &cases {
        let case = format!("delete {name}");
        // What the user looked like before the delete, read back from state.
        let described = ok(
            &router,
            "DescribeUsers",
            &json!({ "UserName": name }),
            &case,
        )
        .await;
        let before = described["Users"][0].clone();
        assert_eq!(
            before["ACLNames"],
            json!(member_of),
            "case {case:?}: expected ACLNames {member_of:?} before the delete, got {described}"
        );

        let deleted = ok(&router, "DeleteUser", &json!({ "UserName": name }), &case).await;
        let user = &deleted["User"];
        assert_eq!(
            user["Name"],
            json!(name),
            "case {case:?}: expected the deleted user's Name {name:?}, got {user}"
        );
        assert_eq!(
            user["Status"],
            json!("deleting"),
            "case {case:?}: a deleted user is reported in the terminal deleting status, \
             got {user}"
        );
        assert_eq!(
            user["ARN"],
            json!(format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:user/{name}")),
            "case {case:?}: the deleted user keeps the ARN it was stored under, got {user}"
        );
        assert_eq!(
            user["AccessString"], before["AccessString"],
            "case {case:?}: the deleted user reports the access string it was stored \
             with ({}), got {user}",
            before["AccessString"]
        );
        assert_eq!(
            user["Authentication"], before["Authentication"],
            "case {case:?}: the deleted user reports the authentication it was stored \
             with ({}), got {user}",
            before["Authentication"]
        );
        assert_eq!(
            user["ACLNames"],
            json!(member_of),
            "case {case:?}: the deleted user carries the membership it held up to the \
             call ({member_of:?}), got {user}"
        );

        // It is gone from the store: a describe by its name answers the modelled
        // not-found fault rather than the user.
        let (status, body) = call(&router, "DescribeUsers", &json!({ "UserName": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a deleted user must not be readable, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "UserNotFoundFault",
            "case {case:?}: expected the modelled not-found fault, got {body}"
        );
        // And gone from every ACL, while every OTHER member stayed put.
        for (acl_name, members) in acl_membership(&router, &case).await {
            assert!(
                !members.iter().any(|member| member == name),
                "case {case:?}: ACL {acl_name:?} must no longer list the deleted user, \
                 got {members:?}"
            );
            assert!(
                members.iter().any(|member| member == BYSTANDER),
                "case {case:?}: ACL {acl_name:?} must still list every other member, \
                 got {members:?}"
            );
        }
        // Deleting it a second time is the modelled not-found fault: it did not come
        // back.
        let (status, body) = call(&router, "DeleteUser", &json!({ "UserName": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second delete must be refused, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "UserNotFoundFault",
            "case {case:?}: expected the modelled not-found fault, got {body}"
        );
    }

    // Every delete above left the one user no case named, in every ACL it belonged to.
    let described = ok(
        &router,
        "DescribeUsers",
        &json!({ "UserName": BYSTANDER }),
        BYSTANDER,
    )
    .await;
    assert_eq!(
        described["Users"][0]["ACLNames"],
        json!(acls),
        "case {BYSTANDER:?}: a user no delete named must still belong to every ACL, \
         got {described}"
    );

    // The user every account already has is not the caller's to delete, under any
    // spelling of its name, and it is still there afterwards.
    for spelling in [DEFAULT_USER, "Default", "DEFAULT"] {
        let (status, body) = call(&router, "DeleteUser", &json!({ "UserName": spelling })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {spelling:?}: a built-in user must refuse the delete, got {status} \
             with {body}"
        );
    }
    let listed = ok(&router, "DescribeUsers", &json!({}), "listing").await;
    assert_eq!(
        user_names(&listed),
        vec![BYSTANDER.to_string(), DEFAULT_USER.to_string()],
        "case \"listing\": the user no delete named and the built-in one are left, got \
         {listed}"
    );
}

/// Naming a user that is not in state is the modelled not-found fault, and such a
/// refusal leaves the store exactly as it was.
#[tokio::test]
async fn deleting_a_user_that_is_not_in_state_is_refused_and_changes_nothing() {
    let router = router();
    create_user(&router, "kept-user").await;
    create_user(&router, "doomed-user").await;
    // An ACL may name a user that was never created (`CreateACL` takes names, not
    // references), so a name appearing in an ACL is NOT a user in state.
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "acl-mixed", "UserNames": ["kept-user", "ghost-user", "doomed-user"] }),
        "acl-mixed",
    )
    .await;
    ok(
        &router,
        "DeleteUser",
        &json!({ "UserName": "doomed-user" }),
        "doomed-user",
    )
    .await;

    // (case, name) — the rows differ in WHY the name is not a user: never used at all,
    // used only inside an ACL, and deleted a moment ago.
    for (case, name) in [
        ("never created", "never-created"),
        ("named by an ACL but never created", "ghost-user"),
        ("deleted a moment ago", "doomed-user"),
    ] {
        let (status, body) = call(&router, "DeleteUser", &json!({ "UserName": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the delete of {name:?} to be refused, got {status} \
             with {body}"
        );
        assert_eq!(
            error_code(&body),
            "UserNotFoundFault",
            "case {case:?}: expected the modelled not-found fault, got {body}"
        );
    }

    // Nothing the refusals named changed: the surviving user is still there, and the
    // ACL still lists exactly the names the delete that DID run left behind — the
    // never-created name among them.
    let listed = ok(&router, "DescribeUsers", &json!({}), "survivors").await;
    assert_eq!(
        user_names(&listed),
        vec![DEFAULT_USER.to_string(), "kept-user".to_string()],
        "case \"survivors\": a refused delete removes no user, got {listed}"
    );
    assert_eq!(
        acl_membership(&router, "survivors").await,
        vec![(
            "acl-mixed".to_string(),
            vec!["kept-user".to_string(), "ghost-user".to_string()],
        )],
        "case \"survivors\": a refused delete must leave every ACL's UserNames \
         untouched, and only the deleted user leaves them"
    );
}
