// SPDX-License-Identifier: CC0-1.0

//! `DeleteGroup` over the wire, driven exactly as an SDK drives it (`POST /`
//! with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `DeleteGroup` removes, from the pool named by
//! `UserPoolId`, the stored group its exact `GroupName` names, and with it every
//! stored user's membership of that group — "when you delete a group, that group
//! no longer contributes to users' `cognito:preferred_group` or
//! `cognito:groups` claims" (this operation's model documentation) — leaving
//! every other group and every other membership of the pool, and every other
//! pool, untouched. A pool it does not name, or a group the named pool does not
//! hold (a second delete of the same group included), is a 404
//! `ResourceNotFoundException` that persists nothing, and a group lookup that
//! found nothing says so in ONE fixed sentence that names no group — the same
//! sentence whatever name was asked for, in every operation that resolves a
//! `GroupName`.
//!
//! The deletion is read back through the implemented readers of the same state:
//! `GetGroup` for the group and `AdminListGroupsForUser` for the membership.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The group every row keeps: what proves a delete touches only the group it
/// names.
const SURVIVING_GROUP: &str = "keeper";

/// The one sentence a group lookup that found nothing says, whatever group name
/// was asked for.
const GROUP_NOT_FOUND_MESSAGE: &str = "Group not found.";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// The `message` of an error body.
fn message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create `group_name` in `user_pool_id` with every optional member set, and
/// return the stored group.
async fn group(router: &MultiServiceRouter, user_pool_id: &str, group_name: &str) -> Value {
    let created = ok(
        router,
        "CreateGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "GroupName": group_name,
            "Description": format!("the {group_name} group"),
            "Precedence": 3,
        }),
    )
    .await;
    created["Group"].clone()
}

/// Create `username` in `user_pool_id`.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
}

/// Add `username` to `group_name`.
async fn add_to_group(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    group_name: &str,
) {
    ok(
        router,
        "AdminAddUserToGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "GroupName": group_name,
        }),
    )
    .await;
}

/// The group names the stored user `username` belongs to, as
/// `AdminListGroupsForUser` reports them.
async fn memberships(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> Vec<String> {
    let listed = ok(
        router,
        "AdminListGroupsForUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    listed["Groups"]
        .as_array()
        .unwrap_or_else(|| panic!("AdminListGroupsForUser must render a list, got {listed}"))
        .iter()
        .map(|group| {
            group["GroupName"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed group names itself, got {group}"))
                .to_string()
        })
        .collect()
}

/// One row of the delete table: the group to delete and the users that belong
/// to it when it is deleted.
struct DeleteCase {
    case: &'static str,
    group_name: String,
    members: Vec<&'static str>,
}

/// The table. Every name satisfies `GroupNameType` (length 1..128, pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) from
/// `smithy_query(query="operation",operation="DeleteGroup")`. The rows differ
/// along the COUNT of memberships the delete has to unassign (none, one,
/// several) and the FORM and LENGTH of the group name (ASCII, punctuation,
/// non-ASCII letters with a number, both published length bounds).
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "a group nobody belongs to",
            group_name: "admins".to_string(),
            members: vec![],
        },
        DeleteCase {
            case: "a group with one member, punctuation in the name",
            group_name: "ops.team-1_a".to_string(),
            members: vec!["alice"],
        },
        DeleteCase {
            case: "a group with three members, non-ASCII letters and a number in the name",
            group_name: "グループ①".to_string(),
            members: vec!["bob", "carol", "dave"],
        },
        DeleteCase {
            case: "a group name at the modelled length maximum 128",
            group_name: "g".repeat(128),
            members: vec!["erin"],
        },
        DeleteCase {
            case: "a group name at the modelled length minimum 1, two members",
            group_name: "x".to_string(),
            members: vec!["frank", "grace"],
        },
    ]
}

#[tokio::test]
async fn deleting_a_group_removes_it_and_unassigns_every_membership_of_it() {
    let router = router();

    for (index, case) in delete_cases().iter().enumerate() {
        let label = case.case;
        let user_pool_id = pool(&router, &format!("delete-pool-{index}")).await;
        let surviving = group(&router, &user_pool_id, SURVIVING_GROUP).await;
        group(&router, &user_pool_id, &case.group_name).await;
        for username in &case.members {
            user(&router, &user_pool_id, username).await;
            add_to_group(&router, &user_pool_id, username, &case.group_name).await;
            add_to_group(&router, &user_pool_id, username, SURVIVING_GROUP).await;
            // `AdminListGroupsForUser` reports in ascending name order.
            let mut expected = vec![SURVIVING_GROUP.to_string(), case.group_name.clone()];
            expected.sort();
            let before = memberships(&router, &user_pool_id, username).await;
            assert_eq!(
                before, expected,
                "case {label:?}: {username} must belong to both groups before the delete, got {before:?}"
            );
        }

        let (status, payload) = call(
            &router,
            "DeleteGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": case.group_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (200, "<no __type>"),
            "case {label:?}: deleting a stored group succeeds, got {status} with {payload}"
        );

        // The group is GONE from stored state: its reader answers a 404.
        let (status, payload) = call(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": case.group_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a deleted group is no longer readable, got {status} with {payload}"
        );
        // The refusal names no group: the same sentence for every row's name.
        assert_eq!(
            message(&payload),
            GROUP_NOT_FOUND_MESSAGE,
            "case {label:?}: expected {GROUP_NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&payload)
        );

        // Every other group of the pool is untouched, member for member.
        let read = ok(
            &router,
            "GetGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": SURVIVING_GROUP }),
        )
        .await;
        assert_eq!(
            read["Group"], surviving,
            "case {label:?}: a delete must leave every other group exactly as it was, got {read}"
        );

        // The membership of the deleted group is gone; the membership of the
        // group that survives is not.
        for username in &case.members {
            let after = memberships(&router, &user_pool_id, username).await;
            assert_eq!(
                after,
                vec![SURVIVING_GROUP.to_string()],
                "case {label:?}: {username} keeps only its membership of the surviving group, got {after:?}"
            );
        }

        // A group the pool no longer holds is a 404, so a second delete of the
        // same name is refused rather than silently succeeding.
        let (status, payload) = call(
            &router,
            "DeleteGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": case.group_name }),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a second delete of the same group is a 404, got {status} with {payload}"
        );
        assert_eq!(
            message(&payload),
            GROUP_NOT_FOUND_MESSAGE,
            "case {label:?}: expected {GROUP_NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&payload)
        );

        // The membership was unassigned, not merely hidden: a group created
        // later under the same name starts with no members.
        let recreated = group(&router, &user_pool_id, &case.group_name).await;
        assert_eq!(
            recreated["GroupName"],
            json!(case.group_name),
            "case {label:?}: the name is free again after the delete, got {recreated}"
        );
        for username in &case.members {
            let after = memberships(&router, &user_pool_id, username).await;
            assert_eq!(
                after,
                vec![SURVIVING_GROUP.to_string()],
                "case {label:?}: a group recreated under a deleted name has no members, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_delete_touches_only_the_pool_it_names() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;

    // The same group name and the same username exist in both pools.
    let mut stored = Vec::new();
    for user_pool_id in [&first_pool, &second_pool] {
        stored.push(group(&router, user_pool_id, "admins").await);
        user(&router, user_pool_id, "alice").await;
        add_to_group(&router, user_pool_id, "alice", "admins").await;
    }

    ok(
        &router,
        "DeleteGroup",
        &json!({ "UserPoolId": first_pool, "GroupName": "admins" }),
    )
    .await;

    let (status, payload) = call(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": first_pool, "GroupName": "admins" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (404, "ResourceNotFoundException"),
        "case \"the pool the delete named\": its group is gone, got {status} with {payload}"
    );
    let first_memberships = memberships(&router, &first_pool, "alice").await;
    assert!(
        first_memberships.is_empty(),
        "case \"the pool the delete named\": its membership is unassigned, got {first_memberships:?}"
    );

    let read = ok(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": second_pool, "GroupName": "admins" }),
    )
    .await;
    assert_eq!(
        read["Group"], stored[1],
        "case \"another pool\": the same group name in another pool survives, got {read}"
    );
    let second_memberships = memberships(&router, &second_pool, "alice").await;
    assert_eq!(
        second_memberships,
        vec!["admins".to_string()],
        "case \"another pool\": its membership survives, got {second_memberships:?}"
    );
}

#[tokio::test]
async fn an_absent_pool_or_group_is_a_404_and_persists_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "notfound-pool").await;
    let stored = group(&router, &user_pool_id, "admins").await;
    user(&router, &user_pool_id, "alice").await;
    add_to_group(&router, &user_pool_id, "alice", "admins").await;

    for (label, body) in [
        (
            "a pool this emulator does not hold",
            json!({ "UserPoolId": "us-east-1_absent", "GroupName": "admins" }),
        ),
        (
            "a group the pool does not hold",
            json!({ "UserPoolId": user_pool_id, "GroupName": "absent-group" }),
        ),
        (
            // `GroupNameType` carries no case-insensitivity trait, so a name
            // differing only in case names a different group.
            "a group name differing only in case",
            json!({ "UserPoolId": user_pool_id, "GroupName": "Admins" }),
        ),
        (
            "a name that is a prefix of a stored one",
            json!({ "UserPoolId": user_pool_id, "GroupName": "admin" }),
        ),
    ] {
        let (status, payload) = call(&router, "DeleteGroup", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            (404, "ResourceNotFoundException"),
            "case {label:?}: must be a 404 ResourceNotFoundException, got {status} with {payload}"
        );
    }

    // A group the pool does not hold is reported by the fixed sentence that
    // names no group, whatever name was asked for; an absent POOL names the
    // pool the request asked for, which the caller sent itself.
    for (label, group_name) in [
        ("a group the pool does not hold", "absent-group"),
        ("a group name differing only in case", "Admins"),
        ("a name that is a prefix of a stored one", "admin"),
    ] {
        let (_, payload) = call(
            &router,
            "DeleteGroup",
            &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
        )
        .await;
        assert_eq!(
            message(&payload),
            GROUP_NOT_FOUND_MESSAGE,
            "case {label:?}: expected {GROUP_NOT_FOUND_MESSAGE:?}, got {:?}",
            message(&payload)
        );
    }

    // None of the refused requests deleted anything.
    let read = ok(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;
    assert_eq!(
        read["Group"], stored,
        "a refused delete must leave the stored group exactly as it was, got {read}"
    );
    let after = memberships(&router, &user_pool_id, "alice").await;
    assert_eq!(
        after,
        vec!["admins".to_string()],
        "a refused delete must leave the stored membership, got {after:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;
    let stored = group(&router, &user_pool_id, "admins").await;

    // One value past each published bound of `DeleteGroup`'s input
    // (`smithy_query(query="operation",operation="DeleteGroup")`): `GroupName`'s
    // length 1..128 and pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, `UserPoolId`'s
    // length 1..55 and pattern `^[\w-]+_[0-9a-zA-Z]+$`, and each required member.
    for (label, body) in [
        (
            "an empty group name, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "GroupName": "" }),
        ),
        (
            "a group name of 129 characters, past the length maximum 128",
            json!({ "UserPoolId": user_pool_id, "GroupName": "g".repeat(129) }),
        ),
        (
            "a group name holding a space, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "GroupName": "two words" }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "GroupName": "admins" }),
        ),
        (
            "a user pool id of 56 characters, past the length maximum 55",
            json!({ "UserPoolId": format!("us-east-1_{}", "a".repeat(46)), "GroupName": "admins" }),
        ),
        (
            "a request missing the required GroupName",
            json!({ "UserPoolId": user_pool_id }),
        ),
        (
            "a request missing the required UserPoolId",
            json!({ "GroupName": "admins" }),
        ),
    ] {
        let (status, payload) = call(&router, "DeleteGroup", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }

    // None of the refused requests deleted anything.
    let read = ok(
        &router,
        "GetGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": "admins" }),
    )
    .await;
    assert_eq!(
        read["Group"], stored,
        "a refused delete must leave the stored group exactly as it was, got {read}"
    );
}
